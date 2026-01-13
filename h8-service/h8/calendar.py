"""Calendar operations."""

import re
import sys
import json
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from typing import Optional, Any

from exchangelib import EWSDateTime, EWSTimeZone, CalendarItem
from exchangelib.account import Account


# Pattern to extract online meeting URLs from body
MEETING_URL_PATTERN = re.compile(
    r"https://(?:"
    r'teams\.microsoft\.com/l/meetup-join/[^\s<>"\']+|'
    r'[^\s<>"\']*\.zoom\.us/[^\s<>"\']+|'
    r'meet\.google\.com/[^\s<>"\']+|'
    r'[^\s<>"\']*webex\.com/[^\s<>"\']+|'
    r'dialin\.teams\.microsoft\.com/[^\s<>"\']+)'
)


def _extract_meeting_url(body: str) -> Optional[str]:
    """Extract online meeting URL from calendar item body."""
    if not body:
        return None
    match = MEETING_URL_PATTERN.search(body)
    return match.group(0) if match else None


def list_events(
    account: Account,
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
) -> list[dict]:
    """List calendar events."""
    tz = ZoneInfo("Europe/Berlin")

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

    # Use .only() to fetch only required fields
    # Include body and is_online_meeting for extracting meeting URLs
    calendar: Any = account.calendar
    query = calendar.view(start=start, end=end).only(
        "id",
        "changekey",
        "subject",
        "start",
        "end",
        "location",
        "organizer",
        "is_all_day",
        "is_cancelled",
        "is_online_meeting",
        "body",
    )

    events = []
    for item in query:
        if not hasattr(item, "start"):
            continue

        # Handle all-day events (date vs datetime)
        start_str = (
            item.start.isoformat()
            if hasattr(item.start, "isoformat")
            else str(item.start)
        )
        end_str = (
            item.end.isoformat() if hasattr(item.end, "isoformat") else str(item.end)
        )

        # Extract meeting URL if it's an online meeting
        meeting_url = None
        is_online = getattr(item, "is_online_meeting", False)
        if is_online and item.body:
            meeting_url = _extract_meeting_url(str(item.body))

        event = {
            "id": item.id,
            "changekey": item.changekey,
            "subject": item.subject,
            "start": start_str,
            "end": end_str,
            "location": item.location,
            "organizer": item.organizer.email_address if item.organizer else None,
            "is_all_day": item.is_all_day,
            "is_cancelled": item.is_cancelled,
        }

        # Only include meeting_url if present
        if meeting_url:
            event["meeting_url"] = meeting_url

        events.append(event)

    return events


def create_event(account: Account, event_data: dict) -> dict:
    """Create a calendar event from JSON data."""
    default_tz = ZoneInfo("Europe/Berlin")

    start_dt = datetime.fromisoformat(event_data["start"])
    end_dt = datetime.fromisoformat(event_data["end"])

    # Add timezone if not present, or convert offset-based timezone to IANA
    if start_dt.tzinfo is None:
        start_dt = start_dt.replace(tzinfo=default_tz)
    else:
        # Convert to IANA timezone to avoid EWS timezone issues
        start_dt = start_dt.astimezone(default_tz)

    if end_dt.tzinfo is None:
        end_dt = end_dt.replace(tzinfo=default_tz)
    else:
        end_dt = end_dt.astimezone(default_tz)

    calendar: Any = account.calendar
    item = CalendarItem(
        account=account,
        folder=calendar,
        subject=event_data["subject"],
        start=EWSDateTime.from_datetime(start_dt),
        end=EWSDateTime.from_datetime(end_dt),
        location=event_data.get("location"),
        body=event_data.get("body"),
    )
    item.save()

    item_start: Any = item.start
    item_end: Any = item.end
    return {
        "id": item.id,
        "changekey": item.changekey,
        "subject": item.subject,
        "start": str(item_start.isoformat()),
        "end": str(item_end.isoformat()),
    }


def delete_event(account: Account, item_id: str) -> dict:
    """Delete a calendar event by ID."""
    from exchangelib import ItemId

    # Find the item by ID
    calendar: Any = account.calendar
    items = list(calendar.filter(id=item_id))

    if not items:
        return {"success": False, "error": "Event not found"}

    items[0].delete()
    return {"success": True, "id": item_id}
