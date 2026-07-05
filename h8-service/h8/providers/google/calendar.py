"""Google Calendar backend mixin: events + free/busy.

Implements the :class:`~h8.providers.base.CalendarBackend` and
:class:`~h8.providers.base.AvailabilityBackend` surface for Google Workspace
using the Calendar API v3 (``events`` and ``freebusy``) reached through
``self._client.calendar()``.

The JSON response shapes are FROZEN to the EWS-derived shapes produced by
``h8/calendar.py``, ``h8/free.py`` and ``h8/people.py`` -- see those modules for
the authoritative dict builders this mixin mirrors. Provider-specific notes:

- ``id`` carries the Calendar event id; ``changekey`` is always ``None``.
- Datetimes are emitted as ISO-8601 with offset in the account/config timezone
  (all-day events emit a bare ``YYYY-MM-DD`` date, matching EWS ``EWSDate``).
- Recurrence is expanded server-side (``singleEvents=True``) so a calendar view
  returns individual occurrences, mirroring the EWS calendar-view expansion.

Slot-finding math (``_merge_busy_times`` / ``_find_slots_from_busy_times``) is a
local re-derivation of the logic in ``h8/free.py`` / ``h8/people.py``: those
modules import ``exchangelib`` at module level and are owned by other agents, so
they cannot be imported here. The duplication is intentional and flagged for a
later dedup pass.
"""

from __future__ import annotations

from datetime import datetime, time, timedelta
from typing import Any, List, Optional
from zoneinfo import ZoneInfo

from h8.config import get_config
from h8.providers.base import (
    CAP_RESOURCES,
    BackendError,
    BackendNotSupported,
)

_PRIMARY = "primary"

# Google Calendar visibility -> EWS-style sensitivity label.
_VISIBILITY_TO_SENSITIVITY = {
    "default": "Normal",
    "public": "Normal",
    "private": "Private",
    "confidential": "Confidential",
}


# ---------------------------------------------------------------------------
# Module-level helpers (no self; prefixed with _)
# ---------------------------------------------------------------------------


def _config_tz() -> ZoneInfo:
    """Return the configured calendar timezone (default ``Europe/Berlin``)."""
    config = get_config()
    return ZoneInfo(config.get("timezone", "Europe/Berlin"))


def _to_rfc3339(dt: datetime) -> str:
    """Render a timezone-aware ``datetime`` as an RFC3339 string for Google."""
    return dt.isoformat()


def _parse_client_datetime(value: str, tz: ZoneInfo) -> datetime:
    """Parse an ISO datetime string the way the EWS impls do.

    Naive inputs are assumed to be in ``tz`` (``replace``); aware inputs keep
    their own offset. Mirrors ``calendar.list_events`` / ``people.*`` parsing.
    """
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        return dt.replace(tzinfo=tz)
    return dt


def _parse_event_datetime(value: str, tz: ZoneInfo) -> datetime:
    """Parse a Google RFC3339 datetime (possibly ``Z``-suffixed) into ``tz``."""
    text = value.replace("Z", "+00:00") if value.endswith("Z") else value
    dt = datetime.fromisoformat(text)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=tz)
    return dt.astimezone(tz)


def _event_endpoint_to_iso(endpoint: dict, tz: ZoneInfo) -> Optional[str]:
    """Map a Google event ``start``/``end`` object to an ISO string.

    All-day endpoints (``{"date": "YYYY-MM-DD"}``) return the bare date; timed
    endpoints (``{"dateTime": ...}``) return ISO-8601 with offset in ``tz``.
    """
    if not endpoint:
        return None
    if endpoint.get("date"):
        return endpoint["date"]
    date_time = endpoint.get("dateTime")
    if date_time:
        return _parse_event_datetime(date_time, tz).isoformat()
    return None


def _extract_meeting_url(event: dict) -> Optional[str]:
    """Best-effort online-meeting URL from ``hangoutLink`` / ``conferenceData``."""
    link = event.get("hangoutLink")
    if link:
        return link
    conf = event.get("conferenceData") or {}
    for entry in conf.get("entryPoints", []) or []:
        if entry.get("entryPointType") == "video" and entry.get("uri"):
            return entry["uri"]
    return None


def _event_to_dict(event: dict, tz: ZoneInfo) -> dict:
    """Map a Google event to the EWS ``calendar.list_events`` dict shape.

    Golden key set (mirrors ``h8/calendar.py``): ``id``, ``changekey``,
    ``subject``, ``start``, ``end``, ``location``, ``organizer``, ``is_all_day``,
    ``is_cancelled``. ``meeting_url`` is added only when present, exactly as EWS.
    """
    start = event.get("start", {}) or {}
    is_all_day = bool(start.get("date"))
    organizer = event.get("organizer") or {}

    result = {
        "id": event.get("id"),
        "changekey": None,
        "subject": event.get("summary"),
        "start": _event_endpoint_to_iso(start, tz),
        "end": _event_endpoint_to_iso(event.get("end", {}) or {}, tz),
        "location": event.get("location"),
        "organizer": organizer.get("email"),
        "is_all_day": is_all_day,
        "is_cancelled": event.get("status") == "cancelled",
    }

    meeting_url = _extract_meeting_url(event)
    if meeting_url:
        result["meeting_url"] = meeting_url

    return result


def _split_attendees(event: dict) -> tuple[list, list]:
    """Return ``(required, optional)`` attendee dicts in the EWS detail shape."""
    required: list = []
    optional: list = []
    for att in event.get("attendees", []) or []:
        entry = {
            "email": att.get("email"),
            "name": att.get("displayName"),
            "response": att.get("responseStatus"),
        }
        if att.get("optional"):
            optional.append(entry)
        else:
            required.append(entry)
    return required, optional


def _my_response(event: dict) -> Optional[str]:
    """Return the authenticated user's ``responseStatus`` for the event."""
    for att in event.get("attendees", []) or []:
        if att.get("self"):
            return att.get("responseStatus")
    organizer = event.get("organizer") or {}
    if organizer.get("self"):
        return "accepted"
    return None


def _parse_recurrence(event: dict) -> Optional[dict]:
    """Best-effort ``{"pattern", "range"}`` from a parent event's RRULE."""
    rules = event.get("recurrence")
    if not rules:
        return None
    rrule = next((r for r in rules if r.upper().startswith("RRULE")), rules[0])
    body = rrule.split(":", 1)[1] if ":" in rrule else rrule
    parts = dict(
        piece.split("=", 1) for piece in body.split(";") if "=" in piece
    )
    pattern = parts.get("FREQ")
    if "UNTIL" in parts:
        range_type = "end_date"
    elif "COUNT" in parts:
        range_type = "numbered"
    else:
        range_type = "no_end"
    return {"pattern": pattern, "range": range_type}


def _event_to_detail(event: dict, tz: ZoneInfo) -> dict:
    """Map a Google event to the EWS ``calendar.get_event`` detail dict shape."""
    start = event.get("start", {}) or {}
    organizer = event.get("organizer") or {}
    required, optional = _split_attendees(event)
    meeting_url = _extract_meeting_url(event)
    visibility = (event.get("visibility") or "default").lower()

    return {
        "success": True,
        "id": event.get("id"),
        "changekey": None,
        "subject": event.get("summary"),
        "start": _event_endpoint_to_iso(start, tz),
        "end": _event_endpoint_to_iso(event.get("end", {}) or {}, tz),
        "location": event.get("location"),
        "organizer": organizer.get("email"),
        "organizer_name": organizer.get("displayName"),
        "is_all_day": bool(start.get("date")),
        "is_cancelled": event.get("status") == "cancelled",
        "is_online_meeting": bool(meeting_url),
        "meeting_url": meeting_url,
        "required_attendees": required,
        "optional_attendees": optional,
        "my_response": _my_response(event),
        "body": event.get("description"),
        "recurrence": _parse_recurrence(event),
        "sensitivity": _VISIBILITY_TO_SENSITIVITY.get(visibility, "Normal"),
        "importance": "Normal",
    }


def _collect_attendees(event_data: dict) -> tuple[list, list]:
    """Extract required/optional attendee email lists from a create payload.

    Mirrors ``calendar.invite_event``: an ``attendees`` field is treated as
    required attendees when no explicit ``required_attendees`` are given.
    """
    required = list(event_data.get("required_attendees", []) or [])
    optional = list(event_data.get("optional_attendees", []) or [])
    if event_data.get("attendees") and not required:
        required = list(event_data["attendees"])
    return required, optional


def _week_window(weeks: int, tz: ZoneInfo) -> tuple[datetime, datetime]:
    """Compute the ``(now, end_of_period)`` window used by the slot finders.

    Re-derived from ``h8/free.py``: the window runs from now to the end of the
    current week (Sunday) plus ``weeks - 1`` additional weeks, at 23:59:59.
    """
    now = datetime.now(tz=tz)
    days_until_sunday = 6 - now.weekday()
    end_of_week = now + timedelta(days=days_until_sunday)
    end_date = end_of_week + timedelta(weeks=weeks - 1)
    end_date = end_date.replace(hour=23, minute=59, second=59)
    return now, end_date


def _merge_busy_times(
    busy_times: List[tuple],
) -> List[tuple]:
    """Merge overlapping ``(start, end)`` intervals (copy of ``people._merge_busy_times``)."""
    if not busy_times:
        return []
    merged: List[tuple] = []
    for busy_start, busy_end in busy_times:
        if merged and busy_start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], busy_end))
        else:
            merged.append((busy_start, busy_end))
    return merged


def _find_slots_from_busy_times(
    merged_busy: List[tuple],
    now: datetime,
    end_date: datetime,
    tz: ZoneInfo,
    start_hour: int,
    end_hour: int,
    exclude_weekends: bool,
    duration_minutes: int,
    limit: Optional[int],
) -> List[dict]:
    """Find free slots given merged busy times.

    Local re-derivation of the identical helper in ``h8/free.py`` /
    ``h8/people.py`` (which cannot be imported without ``exchangelib``). Emits
    the same ``{start, end, duration_minutes, date, day}`` slot dicts.
    """
    free_slots: List[dict] = []
    duration = timedelta(minutes=duration_minutes)

    current_day = now.date()
    end_day = end_date.date()

    while current_day <= end_day:
        if exclude_weekends and current_day.weekday() >= 5:
            current_day += timedelta(days=1)
            continue

        day_start = datetime.combine(current_day, time(start_hour, 0), tzinfo=tz)
        day_end = datetime.combine(current_day, time(end_hour, 0), tzinfo=tz)

        if current_day == now.date():
            day_start = max(day_start, now)
            minutes = day_start.minute
            if minutes % 15 != 0:
                day_start = day_start.replace(
                    minute=(minutes // 15 + 1) * 15 % 60, second=0, microsecond=0
                )
                if minutes >= 45:
                    day_start += timedelta(hours=1)
                    day_start = day_start.replace(minute=0)

        slot_start = day_start

        for busy_start, busy_end in merged_busy:
            if busy_end <= slot_start:
                continue
            if busy_start >= day_end:
                break

            if busy_start > slot_start:
                free_end = min(busy_start, day_end)
                slot_duration = free_end - slot_start
                if slot_duration >= duration:
                    free_slots.append(
                        {
                            "start": slot_start.isoformat(),
                            "end": free_end.isoformat(),
                            "duration_minutes": int(
                                slot_duration.total_seconds() / 60
                            ),
                            "date": current_day.isoformat(),
                            "day": current_day.strftime("%A"),
                        }
                    )
                    if limit and len(free_slots) >= limit:
                        return free_slots

            slot_start = max(slot_start, busy_end)

        if slot_start < day_end:
            slot_duration = day_end - slot_start
            if slot_duration >= duration:
                free_slots.append(
                    {
                        "start": slot_start.isoformat(),
                        "end": day_end.isoformat(),
                        "duration_minutes": int(slot_duration.total_seconds() / 60),
                        "date": current_day.isoformat(),
                        "day": current_day.strftime("%A"),
                    }
                )
                if limit and len(free_slots) >= limit:
                    return free_slots

        current_day += timedelta(days=1)

    return free_slots


def _busy_intervals_from_freebusy(
    response: dict, tz: ZoneInfo, raise_on_error: bool
) -> List[tuple]:
    """Extract merged-ready ``(start, end)`` busy intervals from a freebusy reply.

    When ``raise_on_error`` is set, any per-calendar error block raises a
    :class:`BackendError` describing the likely Workspace domain-visibility
    cause (freebusy for other users only works within the same domain and when
    the target has shared free/busy).
    """
    intervals: List[tuple] = []
    calendars = response.get("calendars", {}) or {}
    for cal_id, cal in calendars.items():
        errors = cal.get("errors")
        if errors and raise_on_error:
            reasons = ", ".join(e.get("reason", "unknown") for e in errors)
            raise BackendError(
                f"Cannot read free/busy for '{cal_id}' ({reasons}). Google "
                "free/busy for other people only works within your Workspace "
                "domain and requires the target to share their calendar."
            )
        for block in cal.get("busy", []) or []:
            start = _parse_event_datetime(block["start"], tz)
            end = _parse_event_datetime(block["end"], tz)
            intervals.append((start, end))
    return intervals


def _slot_params(
    start_hour: Optional[int],
    end_hour: Optional[int],
    exclude_weekends: Optional[bool],
) -> tuple[int, int, bool]:
    """Resolve working-hours / weekend params against config defaults."""
    fs_config = get_config().get("free_slots", {})
    work_start = start_hour if start_hour is not None else fs_config.get("start_hour", 9)
    work_end = end_hour if end_hour is not None else fs_config.get("end_hour", 17)
    skip_weekends = (
        exclude_weekends
        if exclude_weekends is not None
        else fs_config.get("exclude_weekends", True)
    )
    return work_start, work_end, skip_weekends


# ---------------------------------------------------------------------------
# Mixin
# ---------------------------------------------------------------------------


class GoogleCalendarMixin:
    """Google Calendar implementation of the calendar + availability surface."""

    # Provided by GoogleBackend / GoogleClient.
    _client: Any
    account: Any

    # -- internal helpers ---------------------------------------------------

    def _calendar(self) -> Any:
        """Return the Calendar API v3 service."""
        return self._client.calendar()

    def _list_events_raw(
        self,
        time_min: datetime,
        time_max: datetime,
        query: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[dict]:
        """List raw Google events in a window (recurrence expanded, paginated)."""
        events_api = self._calendar().events()
        items: List[dict] = []
        page_token: Optional[str] = None
        while True:
            request = events_api.list(
                calendarId=_PRIMARY,
                timeMin=_to_rfc3339(time_min),
                timeMax=_to_rfc3339(time_max),
                singleEvents=True,
                orderBy="startTime",
                q=query,
                pageToken=page_token,
            )
            response = request.execute()
            items.extend(response.get("items", []) or [])
            if limit is not None and len(items) >= limit:
                return items[:limit]
            page_token = response.get("nextPageToken")
            if not page_token:
                break
        return items

    # -- CalendarBackend ----------------------------------------------------

    def list_events(
        self,
        days: int = 7,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> List[dict]:
        """List calendar events in a view window (mirrors ``calendar.list_events``)."""
        tz = _config_tz()
        start_dt = (
            _parse_client_datetime(from_date, tz)
            if from_date
            else datetime.now(tz=tz)
        )
        end_dt = (
            _parse_client_datetime(to_date, tz)
            if to_date
            else start_dt + timedelta(days=days)
        )
        raw = self._list_events_raw(start_dt, end_dt)
        return [_event_to_dict(event, tz) for event in raw]

    def get_event(self, item_id: str) -> dict:
        """Fetch full details for one event (mirrors ``calendar.get_event``)."""
        tz = _config_tz()
        try:
            event = (
                self._calendar()
                .events()
                .get(calendarId=_PRIMARY, eventId=item_id)
                .execute()
            )
        except Exception as exc:  # noqa: BLE001 - surface as EWS-style error dict
            return {"success": False, "error": f"Failed to get event: {exc}"}
        if not event:
            return {"success": False, "error": "Event not found"}
        return _event_to_detail(event, tz)

    def _build_event_body(self, event_data: dict, tz: ZoneInfo) -> dict:
        """Build the Google event insert body from an EWS-style create payload."""
        is_all_day = bool(event_data.get("is_all_day", False))
        start_dt = _parse_client_datetime(event_data["start"], tz)
        end_dt = _parse_client_datetime(event_data["end"], tz)

        body: dict = {"summary": event_data["subject"]}
        if event_data.get("location"):
            body["location"] = event_data["location"]
        if event_data.get("body"):
            body["description"] = event_data["body"]

        if is_all_day:
            start_date = start_dt.date()
            end_date = end_dt.date()
            if end_date <= start_date:
                end_date = start_date + timedelta(days=1)
            body["start"] = {"date": start_date.isoformat()}
            body["end"] = {"date": end_date.isoformat()}
        else:
            body["start"] = {
                "dateTime": start_dt.isoformat(),
                "timeZone": str(tz),
            }
            body["end"] = {
                "dateTime": end_dt.isoformat(),
                "timeZone": str(tz),
            }
        return body

    def create_event(self, event_data: dict) -> dict:
        """Create an event (mirrors ``calendar.create_event``).

        Attendees are supported defensively: when present the invite is sent
        (``sendUpdates="all"``), otherwise no notifications are sent.
        """
        tz = _config_tz()
        body = self._build_event_body(event_data, tz)

        required, optional = _collect_attendees(event_data)
        attendees = [{"email": e} for e in required] + [
            {"email": e, "optional": True} for e in optional
        ]
        if attendees:
            body["attendees"] = attendees
        send_updates = "all" if attendees else "none"

        created = (
            self._calendar()
            .events()
            .insert(calendarId=_PRIMARY, body=body, sendUpdates=send_updates)
            .execute()
        )
        return {
            "id": created.get("id"),
            "changekey": None,
            "subject": created.get("summary"),
            "start": _event_endpoint_to_iso(created.get("start", {}) or {}, tz),
            "end": _event_endpoint_to_iso(created.get("end", {}) or {}, tz),
        }

    def invite_event(self, event_data: dict) -> dict:
        """Create an event and send meeting invites (mirrors ``calendar.invite_event``)."""
        tz = _config_tz()
        body = self._build_event_body(event_data, tz)

        required, optional = _collect_attendees(event_data)
        attendees = [{"email": e} for e in required] + [
            {"email": e, "optional": True} for e in optional
        ]
        body["attendees"] = attendees

        created = (
            self._calendar()
            .events()
            .insert(calendarId=_PRIMARY, body=body, sendUpdates="all")
            .execute()
        )
        return {
            "id": created.get("id"),
            "changekey": None,
            "subject": created.get("summary"),
            "start": _event_endpoint_to_iso(created.get("start", {}) or {}, tz),
            "end": _event_endpoint_to_iso(created.get("end", {}) or {}, tz),
            "required_attendees": required,
            "optional_attendees": optional,
            "invites_sent": True,
        }

    def delete_event(self, item_id: str) -> dict:
        """Delete an event without notifying attendees (mirrors ``calendar.delete_event``)."""
        try:
            self._calendar().events().delete(
                calendarId=_PRIMARY, eventId=item_id, sendUpdates="none"
            ).execute()
        except Exception as exc:  # noqa: BLE001 - EWS-style error dict
            return {"success": False, "error": f"Failed to delete event: {exc}"}
        return {"success": True, "id": item_id}

    def cancel_event(self, item_id: str, message: Optional[str] = None) -> dict:
        """Cancel a meeting and notify all attendees (mirrors ``calendar.cancel_event``)."""
        events_api = self._calendar().events()
        try:
            event = events_api.get(calendarId=_PRIMARY, eventId=item_id).execute()
        except Exception as exc:  # noqa: BLE001
            return {"success": False, "error": f"Failed to cancel event: {exc}"}
        if not event:
            return {"success": False, "error": "Event not found"}

        organizer = event.get("organizer") or {}
        organizer_email = organizer.get("email")
        account_email = getattr(self.account, "email", None)
        if (
            not organizer.get("self")
            and organizer_email
            and account_email
            and organizer_email.lower() != account_email.lower()
        ):
            return {
                "success": False,
                "error": (
                    "Cannot cancel - you are not the organizer "
                    f"(organizer: {organizer_email})"
                ),
            }

        subject = event.get("summary")
        try:
            events_api.delete(
                calendarId=_PRIMARY, eventId=item_id, sendUpdates="all"
            ).execute()
        except Exception as exc:  # noqa: BLE001
            return {"success": False, "error": f"Failed to cancel event: {exc}"}

        return {
            "success": True,
            "id": item_id,
            "subject": subject,
            "cancellation_sent": True,
            "message": message,
        }

    def search_events(
        self,
        query: str,
        days: int = 90,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
        limit: int = 50,
    ) -> List[dict]:
        """Search events by full text (mirrors ``calendar.search_events``)."""
        tz = _config_tz()
        if from_date:
            start_dt = _parse_client_datetime(from_date, tz)
        else:
            start_dt = datetime.now(tz=tz) - timedelta(days=30)
        if to_date:
            end_dt = _parse_client_datetime(to_date, tz)
        else:
            end_dt = start_dt + timedelta(days=days + 30)

        raw = self._list_events_raw(start_dt, end_dt, query=query, limit=limit)
        return [_event_to_dict(event, tz) for event in raw]

    def list_invites(self, limit: int = 50) -> List[dict]:
        """List pending meeting invites (mirrors ``calendar.list_invites``).

        Freebusy exposes no invite queue, so we scan upcoming calendar events
        for ones where our own attendee ``responseStatus`` is ``needsAction``.
        """
        tz = _config_tz()
        now = datetime.now(tz=tz)
        window_end = now + timedelta(days=90)
        raw = self._list_events_raw(now, window_end)

        invites: List[dict] = []
        for event in raw:
            self_att = next(
                (a for a in event.get("attendees", []) or [] if a.get("self")),
                None,
            )
            if not self_att or self_att.get("responseStatus") != "needsAction":
                continue
            organizer = event.get("organizer") or {}
            start = event.get("start", {}) or {}
            invites.append(
                {
                    "id": event.get("id"),
                    "changekey": None,
                    "subject": event.get("summary"),
                    "start": _event_endpoint_to_iso(start, tz),
                    "end": _event_endpoint_to_iso(event.get("end", {}) or {}, tz),
                    "location": event.get("location"),
                    "organizer": organizer.get("email"),
                    "is_all_day": bool(start.get("date")),
                    "received": event.get("created"),
                    "response_type": self_att.get("responseStatus"),
                }
            )
            if len(invites) >= limit:
                break
        return invites

    def rsvp_event(
        self, item_id: str, response: str, message: Optional[str] = None
    ) -> dict:
        """Respond to a meeting invite (mirrors ``calendar.rsvp_event``).

        Patches the authenticated user's attendee ``responseStatus`` and
        notifies the organizer (``sendUpdates="all"``).
        """
        response_lower = response.lower()
        status_map = {
            "accept": "accepted",
            "decline": "declined",
            "tentative": "tentative",
            "maybe": "tentative",
        }
        new_status = status_map.get(response_lower)
        if new_status is None:
            return {
                "success": False,
                "error": f"Invalid response: {response}. Use accept/decline/tentative",
            }

        events_api = self._calendar().events()
        try:
            event = events_api.get(calendarId=_PRIMARY, eventId=item_id).execute()
        except Exception:  # noqa: BLE001
            return {"success": False, "error": "Meeting invite not found"}
        if not event:
            return {"success": False, "error": "Meeting invite not found"}

        attendees = event.get("attendees", []) or []
        updated = False
        for att in attendees:
            if att.get("self"):
                att["responseStatus"] = new_status
                updated = True
        if not updated:
            return {"success": False, "error": "Item does not support RSVP"}

        events_api.patch(
            calendarId=_PRIMARY,
            eventId=item_id,
            body={"attendees": attendees},
            sendUpdates="all",
        ).execute()

        norm = "tentative" if response_lower == "maybe" else response_lower
        return {
            "success": True,
            "id": item_id,
            "response": norm,
            "subject": event.get("summary"),
        }

    # -- AvailabilityBackend ------------------------------------------------

    def _query_freebusy(
        self,
        item_ids: List[str],
        time_min: datetime,
        time_max: datetime,
        tz: ZoneInfo,
        raise_on_error: bool,
    ) -> List[tuple]:
        """Run ``freebusy.query`` and return merged-ready busy intervals."""
        body = {
            "timeMin": _to_rfc3339(time_min),
            "timeMax": _to_rfc3339(time_max),
            "timeZone": str(tz),
            "items": [{"id": cal_id} for cal_id in item_ids],
        }
        response = self._calendar().freebusy().query(body=body).execute()
        return _busy_intervals_from_freebusy(response, tz, raise_on_error)

    def find_free_slots(
        self,
        weeks: int = 1,
        duration_minutes: int = 30,
        limit: Optional[int] = None,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
        exclude_weekends: Optional[bool] = None,
    ) -> List[dict]:
        """Own free slots via freebusy (mirrors ``free.find_free_slots``)."""
        tz = _config_tz()
        work_start, work_end, skip_weekends = _slot_params(
            start_hour, end_hour, exclude_weekends
        )
        now, end_date = _week_window(weeks, tz)

        busy = self._query_freebusy(
            [_PRIMARY], now, end_date, tz, raise_on_error=False
        )
        busy.sort(key=lambda x: x[0])
        merged = _merge_busy_times(busy)
        return _find_slots_from_busy_times(
            merged, now, end_date, tz, work_start, work_end, skip_weekends,
            duration_minutes, limit,
        )

    def get_person_free_slots(
        self,
        email: str,
        weeks: int = 1,
        duration_minutes: int = 30,
        limit: Optional[int] = None,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
        exclude_weekends: Optional[bool] = None,
    ) -> List[dict]:
        """Another person's free slots via freebusy (mirrors ``people.get_person_free_slots``)."""
        tz = _config_tz()
        work_start, work_end, skip_weekends = _slot_params(
            start_hour, end_hour, exclude_weekends
        )
        now, end_date = _week_window(weeks, tz)

        busy = self._query_freebusy(
            [email], now, end_date, tz, raise_on_error=True
        )
        busy.sort(key=lambda x: x[0])
        merged = _merge_busy_times(busy)
        return _find_slots_from_busy_times(
            merged, now, end_date, tz, work_start, work_end, skip_weekends,
            duration_minutes, limit,
        )

    def find_common_free_slots(
        self,
        emails: List[str],
        weeks: int = 1,
        duration_minutes: int = 30,
        limit: Optional[int] = None,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
        exclude_weekends: Optional[bool] = None,
    ) -> List[dict]:
        """Common free slots across people via freebusy (mirrors ``people.find_common_free_slots``)."""
        tz = _config_tz()
        work_start, work_end, skip_weekends = _slot_params(
            start_hour, end_hour, exclude_weekends
        )
        now, end_date = _week_window(weeks, tz)

        busy = self._query_freebusy(
            list(emails), now, end_date, tz, raise_on_error=True
        )
        busy.sort(key=lambda x: x[0])
        merged = _merge_busy_times(busy)
        return _find_slots_from_busy_times(
            merged, now, end_date, tz, work_start, work_end, skip_weekends,
            duration_minutes, limit,
        )

    def get_person_agenda(
        self,
        email: str,
        days: int = 7,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> List[dict]:
        """Another person's busy times (mirrors ``people.get_person_agenda``).

        Fidelity note: Google freebusy exposes only opaque busy intervals, not
        event subjects/locations. Each interval is returned as a pseudo-event
        with ``status="Busy"`` and ``subject="Busy"``; the richer subject /
        location fields the EWS ``DetailedMerged`` view can surface are not
        available here.
        """
        tz = _config_tz()
        start_dt = (
            _parse_client_datetime(from_date, tz)
            if from_date
            else datetime.now(tz=tz)
        )
        end_dt = (
            _parse_client_datetime(to_date, tz)
            if to_date
            else start_dt + timedelta(days=days)
        )

        busy = self._query_freebusy(
            [email], start_dt, end_dt, tz, raise_on_error=True
        )
        busy.sort(key=lambda x: x[0])
        return [
            {
                "start": start.isoformat(),
                "end": end.isoformat(),
                "status": "Busy",
                "subject": "Busy",
            }
            for start, end in busy
        ]

    # -- Resources: unsupported (defensive; CAP_RESOURCES not advertised) ---

    def resource_free(
        self,
        resources: List[dict],
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
        days: int = 1,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
    ) -> List[dict]:
        """Not supported for Google (requires the Admin SDK)."""
        raise BackendNotSupported(
            "resources require the Google Admin SDK; not supported for provider 'google'",
            capability=CAP_RESOURCES,
        )

    def resource_free_window(
        self, resources: List[dict], from_date: str, to_date: str
    ) -> List[dict]:
        """Not supported for Google (requires the Admin SDK)."""
        raise BackendNotSupported(
            "resources require the Google Admin SDK; not supported for provider 'google'",
            capability=CAP_RESOURCES,
        )

    def resource_agenda(
        self,
        resources: List[dict],
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
        days: int = 1,
    ) -> List[dict]:
        """Not supported for Google (requires the Admin SDK)."""
        raise BackendNotSupported(
            "resources require the Google Admin SDK; not supported for provider 'google'",
            capability=CAP_RESOURCES,
        )
