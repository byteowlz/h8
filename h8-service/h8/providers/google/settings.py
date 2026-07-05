"""Google settings backend: Out-of-Office (via Gmail vacation responder) and
inbox rules (unsupported).

Implements the ``get/set/enable/schedule/disable`` OOF surface of
:class:`~h8.providers.base.SettingsBackend` against the Gmail API's vacation
responder (``users.settings.getVacation`` / ``updateVacation``), mirroring the
response shape produced by the EWS implementation in ``h8/rules_oof.py``.

Gmail has no equivalent of Exchange inbox rules via a public API usable here,
and ``CAP_RULES`` is not in ``GOOGLE_CAPABILITIES`` (see
``h8/providers/google/__init__.py``), so the FastAPI routes already return
HTTP 501 before ever calling these methods. The ``list_rules``/``get_rule``/
etc. overrides below are defense-in-depth for any caller that reaches this
backend directly.

OOF mapping (Gmail vacation responder <-> h8 OOF shape):

- ``state``: ``enableAutoReply=False`` -> ``"Disabled"``; ``True`` with a
  ``startTime``/``endTime`` window -> ``"Scheduled"``; ``True`` with no window
  -> ``"Enabled"``.
- ``start``/``end``: Gmail's epoch-millisecond strings <-> ISO-8601 datetimes.
- ``internal_reply``/``external_reply``: Gmail has no internal/external split
  -- both h8 fields mirror the single ``responseBodyHtml`` (falling back to
  ``responseBodyPlainText``). On write, ``external_reply`` is preferred (falls
  back to ``internal_reply``) as the single body sent to Gmail.
- ``external_audience``: best-effort, driven entirely by ``restrictToDomain``
  (Gmail has no three-way audience split). Reading back: ``restrictToDomain
  == True`` -> ``"None"`` (senders outside the domain truly get no reply,
  which is the closest real-world match to EWS's "None" audience);
  ``False`` -> ``"All"``. Writing: anything other than ``"All"`` (including
  ``"Known"``, which Gmail cannot express) sets ``restrictToDomain = True``.
"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

from googleapiclient.errors import HttpError

from h8.providers.base import (
    CAP_RULES,
    AccountConfig,
    BackendAuthError,
    BackendBusyError,
    BackendError,
    BackendNotSupported,
)
from h8.providers.google.client import GoogleClient


def _execute(request: Any) -> Any:
    """Execute a googleapiclient request, translating HTTP errors.

    401/403 -> :class:`BackendAuthError` (the service layer refreshes and
    retries once). 429/5xx -> :class:`BackendBusyError` (backoff and retry).
    Anything else -> :class:`BackendError`. Mirrors the equivalent helper in
    ``h8.providers.google.contacts``.
    """
    try:
        return request.execute()
    except HttpError as exc:
        status = getattr(getattr(exc, "resp", None), "status", None)
        if status in (401, 403):
            raise BackendAuthError(str(exc)) from exc
        if status == 429 or (status is not None and 500 <= status < 600):
            raise BackendBusyError(str(exc)) from exc
        raise BackendError(str(exc)) from exc


def _epoch_ms_to_iso(value: Any) -> str:
    """Convert a Gmail epoch-millisecond value (str or int) to ISO-8601 UTC."""
    return datetime.fromtimestamp(int(value) / 1000, tz=timezone.utc).isoformat()


def _parse_datetime(value: str) -> datetime:
    """Parse an ISO-8601 (or a couple of common fallback) datetime string.

    Mirrors the parsing tried by ``h8.rules_oof.set_oof_settings``. Naive
    datetimes are assumed UTC.
    """
    try:
        dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        dt = None
        for fmt in ("%Y-%m-%d %H:%M", "%Y-%m-%d"):
            try:
                dt = datetime.strptime(value, fmt)
                break
            except ValueError:
                continue
        if dt is None:
            raise ValueError(f"Unparseable datetime: {value!r}")
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


def _iso_to_epoch_ms(value: str) -> int:
    """Convert an ISO-8601 (or fallback-format) datetime string to Gmail epoch ms."""
    return int(_parse_datetime(value).timestamp() * 1000)


def _restrict_to_domain(external_audience: Optional[str]) -> bool:
    """Best-effort mapping of h8's ``external_audience`` onto Gmail's
    ``restrictToDomain``. See module docstring for the rationale."""
    return (external_audience or "All").strip().lower() != "all"


def _vacation_to_oof_dict(data: Dict[str, Any]) -> Dict[str, Any]:
    """Convert a Gmail ``users.settings.getVacation`` response to the h8 OOF shape."""
    enabled = bool(data.get("enableAutoReply"))
    start_ms = data.get("startTime")
    end_ms = data.get("endTime")
    scheduled = enabled and bool(start_ms or end_ms)
    state = "Scheduled" if scheduled else ("Enabled" if enabled else "Disabled")

    result: Dict[str, Any] = {
        "state": state,
        "external_audience": "None" if data.get("restrictToDomain") else "All",
    }

    if start_ms:
        result["start"] = _epoch_ms_to_iso(start_ms)
    if end_ms:
        result["end"] = _epoch_ms_to_iso(end_ms)

    reply = data.get("responseBodyHtml") or data.get("responseBodyPlainText")
    if reply:
        result["internal_reply"] = reply
        result["external_reply"] = reply

    result["enabled"] = enabled
    result["scheduled"] = scheduled
    return result


def _oof_state_to_vacation_body(
    state: str,
    external_audience: Optional[str],
    start: Optional[str],
    end: Optional[str],
    internal_reply: Optional[str],
    external_reply: Optional[str],
) -> Dict[str, Any]:
    """Build a Gmail ``updateVacation`` request body from the h8 OOF fields."""
    enable = state.strip().lower() in ("enabled", "scheduled")
    body: Dict[str, Any] = {"enableAutoReply": enable}

    if not enable:
        return body

    reply = external_reply or internal_reply or ""
    body["responseBodyHtml"] = reply
    body["restrictToDomain"] = _restrict_to_domain(external_audience)

    if state.strip().lower() == "scheduled" and start and end:
        body["startTime"] = str(_iso_to_epoch_ms(start))
        body["endTime"] = str(_iso_to_epoch_ms(end))

    return body


class GoogleSettingsMixin:
    """Gmail vacation-responder implementation of the ``SettingsBackend`` contract.

    Expects to be mixed into a class providing ``self._client``
    (:class:`~h8.providers.google.client.GoogleClient`) and ``self.account``
    (:class:`~h8.providers.base.AccountConfig`), as
    :class:`~h8.providers.google.GoogleBackend` does. Inbox rules are not
    supported (Gmail has no equivalent surface); ``CAP_RULES`` is not
    advertised so routes 501 before reaching these methods.
    """

    _client: GoogleClient
    account: AccountConfig

    def _settings_resource(self) -> Any:
        """The ``users.settings`` resource of the Gmail API v1 service."""
        return self._client.gmail().users().settings()

    # -- OOF --------------------------------------------------------------

    def get_oof_settings(self) -> Dict[str, Any]:
        """Get Out-of-Office settings via ``users.settings.getVacation``."""
        data = _execute(self._settings_resource().getVacation(userId="me"))
        return _vacation_to_oof_dict(data)

    def set_oof_settings(
        self,
        state: str,
        external_audience: Optional[str] = None,
        start: Optional[str] = None,
        end: Optional[str] = None,
        internal_reply: Optional[str] = None,
        external_reply: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Set Out-of-Office settings via ``users.settings.updateVacation``."""
        body = _oof_state_to_vacation_body(
            state, external_audience, start, end, internal_reply, external_reply
        )
        _execute(self._settings_resource().updateVacation(userId="me", body=body))
        return self.get_oof_settings()

    def enable_oof(
        self,
        internal_reply: str,
        external_reply: Optional[str] = None,
        external_audience: str = "All",
    ) -> Dict[str, Any]:
        """Enable OOF immediately (no schedule window)."""
        return self.set_oof_settings(
            state="Enabled",
            external_audience=external_audience,
            internal_reply=internal_reply,
            external_reply=external_reply or internal_reply,
        )

    def schedule_oof(
        self,
        start: str,
        end: str,
        internal_reply: str,
        external_reply: Optional[str] = None,
        external_audience: str = "All",
    ) -> Dict[str, Any]:
        """Schedule OOF for a future period."""
        return self.set_oof_settings(
            state="Scheduled",
            external_audience=external_audience,
            start=start,
            end=end,
            internal_reply=internal_reply,
            external_reply=external_reply or internal_reply,
        )

    def disable_oof(self) -> Dict[str, Any]:
        """Disable OOF."""
        return self.set_oof_settings(state="Disabled", internal_reply="", external_reply="")

    # -- Inbox rules (unsupported) -----------------------------------------
    #
    # CAP_RULES is not in GOOGLE_CAPABILITIES, so `safe_call_with_retry`
    # already 501s before invoking any of these. They raise explicitly as
    # defense-in-depth for direct backend callers.

    def list_rules(self) -> List[Dict[str, Any]]:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )

    def get_rule(self, rule_id: str) -> Optional[Dict[str, Any]]:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )

    def create_rule(
        self,
        display_name: str,
        priority: int = 1,
        is_enabled: bool = True,
        conditions: Optional[Dict[str, Any]] = None,
        actions: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )

    def update_rule(
        self,
        rule_id: str,
        display_name: Optional[str] = None,
        priority: Optional[int] = None,
        is_enabled: Optional[bool] = None,
        conditions: Optional[Dict[str, Any]] = None,
        actions: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )

    def enable_rule(self, rule_id: str) -> Dict[str, Any]:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )

    def disable_rule(self, rule_id: str) -> Dict[str, Any]:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )

    def delete_rule(self, rule_id: str) -> None:
        raise BackendNotSupported(
            "inbox rules are not supported for provider 'google'", capability=CAP_RULES
        )
