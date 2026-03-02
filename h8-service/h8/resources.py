"""Query availability and bookings for resource groups (rooms, cars, equipment).

Works with named collections of EWS resource mailboxes, querying each
individually to show per-resource availability.
"""

import logging
from datetime import datetime, timedelta
from typing import Any, Optional
from zoneinfo import ZoneInfo

from exchangelib import EWSDateTime
from exchangelib.account import Account

from .config import get_config
from .people import _to_datetime

log = logging.getLogger(__name__)


def resource_free(
    account: Account,
    resources: list[dict],
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    days: int = 1,
    start_hour: Optional[int] = None,
    end_hour: Optional[int] = None,
) -> list[dict]:
    """Check free slots for each resource in a list.

    Args:
        account: The authenticated EWS account
        resources: List of dicts with keys: alias, email, desc (optional)
        from_date: Start date in ISO format (optional)
        to_date: End date in ISO format (optional)
        days: Number of days to look at (default 1)
        start_hour: Start of working hours
        end_hour: End of working hours

    Returns:
        List of per-resource availability dicts with alias, email, desc, free_slots
    """
    config = get_config()
    fs_config = config.get("free_slots", {})
    tz = ZoneInfo(config.get("timezone", "Europe/Berlin"))

    work_start = start_hour if start_hour is not None else fs_config.get("start_hour", 9)
    work_end = end_hour if end_hour is not None else fs_config.get("end_hour", 17)

    if from_date:
        start_dt = datetime.fromisoformat(from_date)
        if start_dt.tzinfo is None:
            start_dt = start_dt.replace(tzinfo=tz)
        # If only a date was given (time is midnight), keep it as start of day
    else:
        start_dt = datetime.now(tz=tz)

    if to_date:
        end_dt = datetime.fromisoformat(to_date)
        if end_dt.tzinfo is None:
            end_dt = end_dt.replace(tzinfo=tz)
        # If only a date was given (time is midnight), set to end of day
        if end_dt.hour == 0 and end_dt.minute == 0 and end_dt.second == 0:
            end_dt = end_dt.replace(hour=23, minute=59, second=59)
    else:
        end_dt = start_dt + timedelta(days=days)
        end_dt = end_dt.replace(hour=23, minute=59, second=59)

    start = EWSDateTime.from_datetime(start_dt)
    end = EWSDateTime.from_datetime(end_dt)

    results = []
    for res in resources:
        email = res["email"]
        alias = res.get("alias", email)
        desc = res.get("desc")

        free_slots = _query_resource_free_slots(
            account, email, start, end, tz, work_start, work_end
        )

        results.append({
            "alias": alias,
            "email": email,
            "desc": desc,
            "free_slots": free_slots,
        })

    return results


def resource_free_window(
    account: Account,
    resources: list[dict],
    from_date: str,
    to_date: str,
) -> list[dict]:
    """Check if each resource is free during a specific time window.

    Args:
        account: The authenticated EWS account
        resources: List of dicts with keys: alias, email, desc (optional)
        from_date: Start of window in ISO format
        to_date: End of window in ISO format

    Returns:
        List of per-resource dicts with alias, email, desc, available (bool)
    """
    config = get_config()
    tz = ZoneInfo(config.get("timezone", "Europe/Berlin"))

    window_start = datetime.fromisoformat(from_date)
    if window_start.tzinfo is None:
        window_start = window_start.replace(tzinfo=tz)
    window_end = datetime.fromisoformat(to_date)
    if window_end.tzinfo is None:
        window_end = window_end.replace(tzinfo=tz)

    start = EWSDateTime.from_datetime(window_start)
    end = EWSDateTime.from_datetime(window_end)

    results = []
    for res in resources:
        email = res["email"]
        alias = res.get("alias", email)
        desc = res.get("desc")

        busy_times = _query_resource_busy_times(account, email, start, end, tz)
        # Resource is available if no busy time overlaps the window
        available = not any(
            bs < window_end and be > window_start for bs, be in busy_times
        )

        results.append({
            "alias": alias,
            "email": email,
            "desc": desc,
            "available": available,
        })

    return results


def resource_agenda(
    account: Account,
    resources: list[dict],
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    days: int = 1,
) -> list[dict]:
    """Get bookings/events for each resource in a list.

    Args:
        account: The authenticated EWS account
        resources: List of dicts with keys: alias, email, desc (optional)
        from_date: Start date in ISO format (optional)
        to_date: End date in ISO format (optional)
        days: Number of days to look at (default 1)

    Returns:
        List of per-resource dicts with alias, email, desc, events
    """
    config = get_config()
    tz = ZoneInfo(config.get("timezone", "Europe/Berlin"))

    if from_date:
        start_dt = datetime.fromisoformat(from_date)
        if start_dt.tzinfo is None:
            start_dt = start_dt.replace(tzinfo=tz)
    else:
        start_dt = datetime.now(tz=tz)

    if to_date:
        end_dt = datetime.fromisoformat(to_date)
        if end_dt.tzinfo is None:
            end_dt = end_dt.replace(tzinfo=tz)
        if end_dt.hour == 0 and end_dt.minute == 0 and end_dt.second == 0:
            end_dt = end_dt.replace(hour=23, minute=59, second=59)
    else:
        end_dt = start_dt + timedelta(days=days)
        end_dt = end_dt.replace(hour=23, minute=59, second=59)

    start = EWSDateTime.from_datetime(start_dt)
    end = EWSDateTime.from_datetime(end_dt)

    results = []
    for res in resources:
        email = res["email"]
        alias = res.get("alias", email)
        desc = res.get("desc")

        events = _query_resource_events(account, email, start, end, tz)

        results.append({
            "alias": alias,
            "email": email,
            "desc": desc,
            "events": events,
        })

    return results


def _query_resource_free_slots(
    account: Account,
    email: str,
    start: EWSDateTime,
    end: EWSDateTime,
    tz: ZoneInfo,
    work_start_hour: int,
    work_end_hour: int,
) -> list[dict]:
    """Query free slots for a single resource email."""
    from datetime import time

    busy_times = _query_resource_busy_times(account, email, start, end, tz)

    # Sort and merge
    busy_times.sort(key=lambda x: x[0])
    merged: list[tuple[datetime, datetime]] = []
    for bs, be in busy_times:
        if merged and bs <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], be))
        else:
            merged.append((bs, be))

    # Find free slots within working hours
    free_slots: list[dict] = []
    start_dt = datetime(start.year, start.month, start.day, tzinfo=tz)
    end_dt = datetime(end.year, end.month, end.day, 23, 59, 59, tzinfo=tz)

    current_day = start_dt.date()
    end_day = end_dt.date()

    while current_day <= end_day:
        day_start = datetime.combine(current_day, time(work_start_hour, 0), tzinfo=tz)
        day_end = datetime.combine(current_day, time(work_end_hour, 0), tzinfo=tz)

        now = datetime.now(tz=tz)
        if current_day == now.date():
            day_start = max(day_start, now)

        slot_start = day_start

        for bs, be in merged:
            if be <= slot_start:
                continue
            if bs >= day_end:
                break
            if bs > slot_start:
                free_end = min(bs, day_end)
                free_slots.append({
                    "start": slot_start.isoformat(),
                    "end": free_end.isoformat(),
                    "date": current_day.isoformat(),
                    "day": current_day.strftime("%A"),
                })
            slot_start = max(slot_start, be)

        if slot_start < day_end:
            free_slots.append({
                "start": slot_start.isoformat(),
                "end": day_end.isoformat(),
                "date": current_day.isoformat(),
                "day": current_day.strftime("%A"),
            })

        current_day += timedelta(days=1)

    return free_slots


def _query_resource_busy_times(
    account: Account,
    email: str,
    start: EWSDateTime,
    end: EWSDateTime,
    tz: ZoneInfo,
) -> list[tuple[datetime, datetime]]:
    """Get busy times for a single resource email via GetUserAvailability."""
    try:
        results = list(
            account.protocol.get_free_busy_info(
                accounts=[(email, "Required", False)],
                start=start,
                end=end,
                merged_free_busy_interval=15,
                requested_view="DetailedMerged",
            )
        )
    except Exception as exc:
        log.warning("Failed to query free/busy for %s: %s", email, exc)
        return []

    busy_times: list[tuple[datetime, datetime]] = []
    if results:
        fb_view: Any = results[0]
        if hasattr(fb_view, "calendar_events") and fb_view.calendar_events:
            for event in fb_view.calendar_events:
                if event.start and event.end:
                    event_start = _to_datetime(event.start, tz)
                    event_end = _to_datetime(event.end, tz)
                    busy_times.append((event_start, event_end))

    return busy_times


def _query_resource_events(
    account: Account,
    email: str,
    start: EWSDateTime,
    end: EWSDateTime,
    tz: ZoneInfo,
) -> list[dict]:
    """Get calendar events for a single resource email."""
    try:
        results = list(
            account.protocol.get_free_busy_info(
                accounts=[(email, "Required", False)],
                start=start,
                end=end,
                merged_free_busy_interval=30,
                requested_view="DetailedMerged",
            )
        )
    except Exception as exc:
        log.warning("Failed to query events for %s: %s", email, exc)
        return []

    events: list[dict] = []
    if results:
        fb_view: Any = results[0]
        if hasattr(fb_view, "calendar_events") and fb_view.calendar_events:
            for event in fb_view.calendar_events:
                event_dict: dict[str, Any] = {
                    "start": event.start.isoformat() if event.start else None,
                    "end": event.end.isoformat() if event.end else None,
                    "status": event.free_busy_status
                    if hasattr(event, "free_busy_status")
                    else "Busy",
                }
                if hasattr(event, "subject") and event.subject:
                    event_dict["subject"] = event.subject
                if hasattr(event, "location") and event.location:
                    event_dict["location"] = event.location
                events.append(event_dict)

    return events
