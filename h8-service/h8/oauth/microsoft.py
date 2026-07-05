"""Microsoft OAuth via MSAL: silent refresh and device-code login.

A single :class:`msal.PublicClientApplication` is maintained per
``(client_id, tenant, email)`` with a :class:`msal.SerializableTokenCache`
persisted through :class:`~h8.oauth.store.TokenStore` under the key
``ms:{email}``. The cache is (re)loaded before every acquisition and persisted
afterwards whenever ``cache.has_state_changed`` is set.

Dual audience: one login yields one refresh token in the shared cache. MSAL
mints a distinct, per-scope access token from that single refresh token for each
audience -- EWS (``https://outlook.office365.com``) and Microsoft Graph -- so
callers never manage refresh tokens directly. :func:`get_ms_token` simply asks
for the scopes of the requested resource and MSAL returns (or silently refreshes)
the matching access token.

Configuration: set ``client_id`` on the account (an Azure AD app registration
with the required delegated permissions and public-client / device-code flow
enabled). ``DEFAULT_CLIENT_ID`` is intentionally empty and must be populated
with the operator's own app registration id before device login can succeed.
"""

import logging
import threading
import time
import uuid
from dataclasses import dataclass
from typing import Literal, Optional

import msal

from h8.oauth import LoginRequired
from h8.oauth._account import AccountLike
from h8.oauth.store import TokenStore, get_default_store

log = logging.getLogger(__name__)

# NEEDS OPERATOR CONFIGURATION: replace with your Azure AD (Entra) public-client
# application id, or set `client_id` per account in config.toml. Left empty on
# purpose so an unconfigured deployment fails loudly with a helpful message.
DEFAULT_CLIENT_ID = ""

DEFAULT_TENANT = "organizations"
AUTHORITY_TEMPLATE = "https://login.microsoftonline.com/{tenant}"

EWS_SCOPES = ["https://outlook.office365.com/EWS.AccessAsUser.All"]
# MSAL adds `offline_access` itself; do not list it here.
GRAPH_SCOPES = [
    "Mail.ReadWrite",
    "Mail.Send",
    "Calendars.ReadWrite",
    "Contacts.ReadWrite",
    "MailboxSettings.ReadWrite",
    "People.Read",
    "User.ReadBasic.All",
]

# Scopes requested during interactive login. Graph scopes are requested so a
# forward-looking Graph backend works; the shared refresh token also serves EWS.
LOGIN_SCOPES = GRAPH_SCOPES

Resource = Literal["ews", "graph"]


@dataclass
class AccessToken:
    """An acquired access token and its absolute expiry (epoch seconds)."""

    token: str
    expires_at: float


@dataclass
class DeviceLogin:
    """Device-code flow start details handed back to the caller/UI."""

    session_id: str
    verification_url: str
    user_code: str
    expires_at: float


_apps: dict[tuple[str, str, str], tuple[msal.PublicClientApplication, msal.SerializableTokenCache]] = {}
_apps_lock = threading.Lock()

_sessions: dict[str, dict] = {}
_sessions_lock = threading.Lock()


def _cache_key(email: str) -> str:
    return f"ms:{email}"


def _resolve_client_id(account: AccountLike) -> str:
    client_id = (account.client_id or "").strip() or DEFAULT_CLIENT_ID
    if not client_id:
        raise LoginRequired(
            f"No Microsoft client_id configured for account '{account.email}'. "
            "Register a public-client application in Azure AD (Entra) and set "
            "its id as `client_id` in the account config (or populate "
            "DEFAULT_CLIENT_ID)."
        )
    return client_id


def _resolve_tenant(account: AccountLike) -> str:
    return (account.tenant or "").strip() or DEFAULT_TENANT


def _scopes_for(resource: Resource) -> list[str]:
    if resource == "ews":
        return list(EWS_SCOPES)
    if resource == "graph":
        return list(GRAPH_SCOPES)
    raise ValueError(f"Unknown resource {resource!r}; expected 'ews' or 'graph'")


def _get_store() -> TokenStore:
    return get_default_store()


def _persist_cache(store: TokenStore, email: str, cache: msal.SerializableTokenCache) -> None:
    if cache.has_state_changed:
        store.set(_cache_key(email), cache.serialize())


def _get_app(account: AccountLike) -> tuple[msal.PublicClientApplication, msal.SerializableTokenCache]:
    """Return the (app, cache) for ``account``, loading persisted cache state."""
    client_id = _resolve_client_id(account)
    tenant = _resolve_tenant(account)
    email = account.email
    key = (client_id, tenant, email)

    with _apps_lock:
        entry = _apps.get(key)
        if entry is None:
            cache = msal.SerializableTokenCache()
            app = msal.PublicClientApplication(
                client_id,
                authority=AUTHORITY_TEMPLATE.format(tenant=tenant),
                token_cache=cache,
            )
            entry = (app, cache)
            _apps[key] = entry
        app, cache = entry

    raw = _get_store().get(_cache_key(email))
    if raw:
        cache.deserialize(raw)
    return app, cache


def get_ms_token(account: AccountLike, resource: Resource) -> AccessToken:
    """Return an access token for ``resource`` (``"ews"`` or ``"graph"``).

    Tries ``acquire_token_silent`` against the cached account first. Raises
    :class:`~h8.oauth.LoginRequired` when there is no cached account or the
    silent acquisition fails (interactive login required).
    """
    scopes = _scopes_for(resource)
    app, cache = _get_app(account)

    accounts = app.get_accounts(username=account.email)
    if not accounts:
        raise LoginRequired(
            f"No cached Microsoft credentials for '{account.email}'. "
            "Run the device-code login first."
        )

    result = app.acquire_token_silent(scopes, account=accounts[0])
    _persist_cache(_get_store(), account.email, cache)

    if not result or "access_token" not in result:
        detail = ""
        if result and result.get("error_description"):
            detail = f" ({result['error_description']})"
        raise LoginRequired(
            f"Silent token acquisition failed for '{account.email}'{detail}. "
            "Interactive login required."
        )

    expires_in = float(result.get("expires_in", 0))
    return AccessToken(token=result["access_token"], expires_at=time.time() + expires_in)


def _set_session(session_id: str, status: str, detail: Optional[str]) -> None:
    with _sessions_lock:
        session = _sessions.get(session_id)
        if session is not None:
            session["status"] = status
            session["detail"] = detail


def start_device_login(account: AccountLike) -> DeviceLogin:
    """Begin the device-code flow; the blocking wait runs in a daemon thread.

    Poll :func:`poll_device_login` with the returned ``session_id`` until it
    reports ``"done"`` or an error.
    """
    app, cache = _get_app(account)
    flow = app.initiate_device_flow(scopes=LOGIN_SCOPES)
    if "user_code" not in flow:
        detail = flow.get("error_description") or flow.get("error") or str(flow)
        raise LoginRequired(f"Failed to start device flow for '{account.email}': {detail}")

    session_id = uuid.uuid4().hex
    expires_at = time.time() + float(flow.get("expires_in", 900))
    with _sessions_lock:
        _sessions[session_id] = {"status": "pending", "detail": None, "email": account.email}

    def _worker() -> None:
        try:
            result = app.acquire_token_by_device_flow(flow)
            if "access_token" in result:
                _persist_cache(_get_store(), account.email, cache)
                _set_session(session_id, "done", None)
            else:
                detail = result.get("error_description") or result.get("error") or "unknown error"
                _set_session(session_id, "error", detail)
        except Exception as exc:  # noqa: BLE001 - surface any failure to the poller
            log.exception("Device login failed for %s", account.email)
            _set_session(session_id, "error", str(exc))

    thread = threading.Thread(target=_worker, name=f"ms-device-{session_id}", daemon=True)
    thread.start()

    return DeviceLogin(
        session_id=session_id,
        verification_url=flow.get("verification_uri", ""),
        user_code=flow["user_code"],
        expires_at=expires_at,
    )


def poll_device_login(session_id: str) -> str:
    """Return ``"pending"``, ``"done"`` or ``"error: <detail>"`` for a session."""
    with _sessions_lock:
        session = _sessions.get(session_id)
    if session is None:
        return "error: unknown session"
    if session["status"] == "error":
        return f"error: {session['detail']}"
    return session["status"]


def delete_cache(account: AccountLike) -> None:
    """Remove any persisted MSAL cache for ``account`` and drop the live app."""
    _get_store().delete(_cache_key(account.email))
    with _apps_lock:
        for key in [k for k in _apps if k[2] == account.email]:
            del _apps[key]
