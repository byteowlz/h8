"""Operations for viewing other people's calendar information.

Uses EWS GetUserAvailability to fetch free/busy information for users
who have shared their calendars.
"""

from datetime import datetime, timedelta, time
from zoneinfo import ZoneInfo
from typing import Optional, Any

from exchangelib import EWSDateTime
from exchangelib.account import Account

from .config import get_config


def get_person_agenda(
    account: Account,
    email: str,
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
) -> list[dict]:
    """Get calendar events/busy times for another person.

    Uses EWS GetUserAvailability which returns free/busy information
    for users who have shared their calendars.

    Args:
        account: The authenticated EWS account (your account)
        email: Email address of the person to query
        days: Number of days to look at (default 7)
        from_date: Start date in ISO format (optional)
        to_date: End date in ISO format (optional)

    Returns:
        List of event dictionaries with start, end, subject (if available), etc.
    """
    config = get_config()
    tz = ZoneInfo(config.get("timezone", "Europe/Berlin"))

    if from_date:
        start_dt = datetime.fromisoformat(from_date).replace(tzinfo=tz)
    else:
        start_dt = datetime.now(tz=tz)

    if to_date:
        end_dt = datetime.fromisoformat(to_date).replace(tzinfo=tz)
    else:
        end_dt = start_dt + timedelta(days=days)

    start = EWSDateTime.from_datetime(start_dt)
    end = EWSDateTime.from_datetime(end_dt)

    # Use protocol.get_free_busy_info to get availability
    # accounts param is list of (account_or_email, attendee_type, exclude_conflicts)
    results = list(
        account.protocol.get_free_busy_info(
            accounts=[(email, "Required", False)],
            start=start,
            end=end,
            merged_free_busy_interval=30,
            requested_view="DetailedMerged",
        )
    )

    events = []
    if results:
        fb_view: Any = results[0]
        # FreeBusyView has calendar_events attribute
        if hasattr(fb_view, "calendar_events") and fb_view.calendar_events:
            for event in fb_view.calendar_events:
                event_dict = {
                    "start": event.start.isoformat() if event.start else None,
                    "end": event.end.isoformat() if event.end else None,
                    "status": event.free_busy_status
                    if hasattr(event, "free_busy_status")
                    else "Busy",
                }
                # CalendarEvent may have subject and location if detailed info is available
                if hasattr(event, "subject") and event.subject:
                    event_dict["subject"] = event.subject
                if hasattr(event, "location") and event.location:
                    event_dict["location"] = event.location
                events.append(event_dict)

    return events


def get_person_free_slots(
    account: Account,
    email: str,
    weeks: int = 1,
    duration_minutes: int = 30,
    limit: Optional[int] = None,
    start_hour: Optional[int] = None,
    end_hour: Optional[int] = None,
    exclude_weekends: Optional[bool] = None,
) -> list[dict]:
    """Find free slots in another person's calendar.

    Args:
        account: The authenticated EWS account (your account)
        email: Email address of the person to query
        weeks: Number of weeks to look at (1 = current week until Sunday)
        duration_minutes: Minimum duration of free slot in minutes
        limit: Maximum number of slots to return
        start_hour: Start of working hours (default from config)
        end_hour: End of working hours (default from config)
        exclude_weekends: Whether to exclude weekends (default from config)

    Returns:
        List of free slot dictionaries with start, end, duration_minutes
    """
    config = get_config()
    fs_config = config.get("free_slots", {})

    # Use config defaults if not specified
    work_start_hour = (
        start_hour if start_hour is not None else fs_config.get("start_hour", 9)
    )
    work_end_hour = end_hour if end_hour is not None else fs_config.get("end_hour", 17)
    skip_weekends = (
        exclude_weekends
        if exclude_weekends is not None
        else fs_config.get("exclude_weekends", True)
    )

    tz = ZoneInfo(config.get("timezone", "Europe/Berlin"))
    now = datetime.now(tz=tz)

    # Calculate end of period
    days_until_sunday = 6 - now.weekday()
    end_of_week = now + timedelta(days=days_until_sunday)
    end_date = end_of_week + timedelta(weeks=weeks - 1)
    end_date = end_date.replace(hour=23, minute=59, second=59)

    start = EWSDateTime.from_datetime(now)
    end = EWSDateTime.from_datetime(end_date)

    # Get free/busy info
    results = list(
        account.protocol.get_free_busy_info(
            accounts=[(email, "Required", False)],
            start=start,
            end=end,
            merged_free_busy_interval=15,  # Higher resolution for better slot finding
            requested_view="DetailedMerged",
        )
    )

    # Extract busy times
    busy_times: list[tuple[datetime, datetime]] = []
    if results:
        fb_view: Any = results[0]
        if hasattr(fb_view, "calendar_events") and fb_view.calendar_events:
            for event in fb_view.calendar_events:
                if event.start and event.end:
                    # Convert to datetime
                    event_start = _to_datetime(event.start, tz)
                    event_end = _to_datetime(event.end, tz)
                    busy_times.append((event_start, event_end))

    # Sort and merge busy times
    busy_times.sort(key=lambda x: x[0])
    merged_busy = _merge_busy_times(busy_times)

    # Find free slots
    return _find_slots_from_busy_times(
        merged_busy,
        now,
        end_date,
        tz,
        work_start_hour,
        work_end_hour,
        skip_weekends,
        duration_minutes,
        limit,
    )


def find_common_free_slots(
    account: Account,
    emails: list[str],
    weeks: int = 1,
    duration_minutes: int = 30,
    limit: Optional[int] = None,
    start_hour: Optional[int] = None,
    end_hour: Optional[int] = None,
    exclude_weekends: Optional[bool] = None,
) -> list[dict]:
    """Find common free slots across multiple people's calendars.

    Args:
        account: The authenticated EWS account (your account)
        emails: List of email addresses to check
        weeks: Number of weeks to look at
        duration_minutes: Minimum duration of free slot in minutes
        limit: Maximum number of slots to return
        start_hour: Start of working hours
        end_hour: End of working hours
        exclude_weekends: Whether to exclude weekends

    Returns:
        List of common free slot dictionaries
    """
    config = get_config()
    fs_config = config.get("free_slots", {})

    work_start_hour = (
        start_hour if start_hour is not None else fs_config.get("start_hour", 9)
    )
    work_end_hour = end_hour if end_hour is not None else fs_config.get("end_hour", 17)
    skip_weekends = (
        exclude_weekends
        if exclude_weekends is not None
        else fs_config.get("exclude_weekends", True)
    )

    tz = ZoneInfo(config.get("timezone", "Europe/Berlin"))
    now = datetime.now(tz=tz)

    days_until_sunday = 6 - now.weekday()
    end_of_week = now + timedelta(days=days_until_sunday)
    end_date = end_of_week + timedelta(weeks=weeks - 1)
    end_date = end_date.replace(hour=23, minute=59, second=59)

    start = EWSDateTime.from_datetime(now)
    end = EWSDateTime.from_datetime(end_date)

    # Query all people at once (more efficient)
    account_tuples = [(email, "Required", False) for email in emails]
    results = list(
        account.protocol.get_free_busy_info(
            accounts=account_tuples,
            start=start,
            end=end,
            merged_free_busy_interval=15,
            requested_view="DetailedMerged",
        )
    )

    # Collect all busy times from all people
    all_busy_times: list[tuple[datetime, datetime]] = []
    for fb_view_item in results:
        fb_view: Any = fb_view_item
        if hasattr(fb_view, "calendar_events") and fb_view.calendar_events:
            for event in fb_view.calendar_events:
                if event.start and event.end:
                    event_start = _to_datetime(event.start, tz)
                    event_end = _to_datetime(event.end, tz)
                    all_busy_times.append((event_start, event_end))

    # Sort and merge all busy times
    all_busy_times.sort(key=lambda x: x[0])
    merged_busy = _merge_busy_times(all_busy_times)

    # Find free slots (times when ALL people are free)
    return _find_slots_from_busy_times(
        merged_busy,
        now,
        end_date,
        tz,
        work_start_hour,
        work_end_hour,
        skip_weekends,
        duration_minutes,
        limit,
    )


def _to_datetime(ews_dt: Any, tz: ZoneInfo) -> datetime:
    """Convert EWS datetime to Python datetime."""
    if hasattr(ews_dt, "hour"):
        # It's a datetime
        if hasattr(ews_dt, "tzinfo") and ews_dt.tzinfo is not None:
            return datetime(
                ews_dt.year,
                ews_dt.month,
                ews_dt.day,
                ews_dt.hour,
                ews_dt.minute,
                ews_dt.second,
                tzinfo=ews_dt.tzinfo,
            ).astimezone(tz)
        else:
            return datetime(
                ews_dt.year,
                ews_dt.month,
                ews_dt.day,
                ews_dt.hour,
                ews_dt.minute,
                ews_dt.second,
                tzinfo=tz,
            )
    else:
        # It's a date (all-day event)
        return datetime.combine(ews_dt, time(0, 0), tzinfo=tz)


def _merge_busy_times(
    busy_times: list[tuple[datetime, datetime]],
) -> list[tuple[datetime, datetime]]:
    """Merge overlapping busy time intervals."""
    if not busy_times:
        return []

    merged: list[tuple[datetime, datetime]] = []
    for busy_start, busy_end in busy_times:
        if merged and busy_start <= merged[-1][1]:
            # Overlapping, extend the last busy period
            merged[-1] = (merged[-1][0], max(merged[-1][1], busy_end))
        else:
            merged.append((busy_start, busy_end))

    return merged


def _find_slots_from_busy_times(
    merged_busy: list[tuple[datetime, datetime]],
    now: datetime,
    end_date: datetime,
    tz: ZoneInfo,
    start_hour: int,
    end_hour: int,
    exclude_weekends: bool,
    duration_minutes: int,
    limit: Optional[int],
) -> list[dict]:
    """Find free slots given merged busy times."""
    free_slots: list[dict] = []
    duration = timedelta(minutes=duration_minutes)

    current_day = now.date()
    end_day = end_date.date()

    while current_day <= end_day:
        # Skip weekends if configured
        if exclude_weekends and current_day.weekday() >= 5:
            current_day += timedelta(days=1)
            continue

        # Define working hours for this day
        day_start = datetime.combine(current_day, time(start_hour, 0), tzinfo=tz)
        day_end = datetime.combine(current_day, time(end_hour, 0), tzinfo=tz)

        # If it's today, start from now (but not before start_hour)
        if current_day == now.date():
            day_start = max(day_start, now)
            # Round up to next 15-minute slot
            minutes = day_start.minute
            if minutes % 15 != 0:
                day_start = day_start.replace(
                    minute=(minutes // 15 + 1) * 15 % 60, second=0, microsecond=0
                )
                if minutes >= 45:
                    day_start += timedelta(hours=1)
                    day_start = day_start.replace(minute=0)

        # Find free slots in this day
        slot_start = day_start

        for busy_start, busy_end in merged_busy:
            # Skip busy times before our current slot
            if busy_end <= slot_start:
                continue
            # Stop if busy time is after this day
            if busy_start >= day_end:
                break

            # Check if there's a free slot before this busy time
            if busy_start > slot_start:
                free_end = min(busy_start, day_end)
                slot_duration = free_end - slot_start

                if slot_duration >= duration:
                    free_slots.append(
                        {
                            "start": slot_start.isoformat(),
                            "end": free_end.isoformat(),
                            "duration_minutes": int(slot_duration.total_seconds() / 60),
                            "date": current_day.isoformat(),
                            "day": current_day.strftime("%A"),
                        }
                    )

                    if limit and len(free_slots) >= limit:
                        return free_slots

            # Move slot_start past this busy time
            slot_start = max(slot_start, busy_end)

        # Check for free slot after all busy times
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
