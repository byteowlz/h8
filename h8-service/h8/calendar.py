"""Calendar operations."""

import re
import sys
import json
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from typing import Optional, Any

from exchangelib import EWSDateTime, EWSTimeZone, CalendarItem, Attendee, Mailbox
from exchangelib.account import Account
from exchangelib.items import MeetingRequest


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

    # Fetch item by ID using account.fetch() - EWS IDs are globally unique
    try:
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"success": False, "error": "Event not found"}

        items[0].delete()
        return {"success": True, "id": item_id}
    except Exception as e:
        return {"success": False, "error": f"Failed to delete event: {e}"}


def search_events(
    account: Account,
    query: str,
    days: int = 90,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    limit: int = 50,
) -> list[dict]:
    """Search calendar events by subject, location, or body.

    Args:
        account: EWS account
        query: Search string (case-insensitive substring match)
        days: Number of days to search (default 90)
        from_date: Optional start date (ISO format)
        to_date: Optional end date (ISO format)
        limit: Maximum number of results to return

    Returns:
        List of matching event dictionaries
    """
    tz = ZoneInfo("Europe/Berlin")

    if from_date:
        start_dt = datetime.fromisoformat(from_date).replace(tzinfo=tz)
    else:
        # Search from 30 days in the past by default
        start_dt = datetime.now(tz=tz) - timedelta(days=30)

    if to_date:
        end_dt = datetime.fromisoformat(to_date).replace(tzinfo=tz)
    else:
        end_dt = start_dt + timedelta(days=days + 30)

    start = EWSDateTime.from_datetime(start_dt)
    end = EWSDateTime.from_datetime(end_dt)

    # Use calendar view to get events in the date range
    calendar: Any = account.calendar
    view = calendar.view(start=start, end=end).only(
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

    query_lower = query.lower()
    events = []

    for item in view:
        if not hasattr(item, "start"):
            continue

        # Check if query matches subject, location, or body
        subject = (item.subject or "").lower()
        location = (item.location or "").lower()
        body = (str(item.body) if item.body else "").lower()

        if (
            query_lower not in subject
            and query_lower not in location
            and query_lower not in body
        ):
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

        if meeting_url:
            event["meeting_url"] = meeting_url

        events.append(event)

        if len(events) >= limit:
            break

    return events


def invite_event(account: Account, event_data: dict) -> dict:
    """Create a calendar event and send meeting invites to attendees.

    Args:
        account: EWS account
        event_data: Dict with subject, start, end, and attendees (required/optional)

    Returns:
        Dict with event details and invite status
    """
    default_tz = ZoneInfo("Europe/Berlin")

    start_dt = datetime.fromisoformat(event_data["start"])
    end_dt = datetime.fromisoformat(event_data["end"])

    # Add timezone if not present
    if start_dt.tzinfo is None:
        start_dt = start_dt.replace(tzinfo=default_tz)
    else:
        start_dt = start_dt.astimezone(default_tz)

    if end_dt.tzinfo is None:
        end_dt = end_dt.replace(tzinfo=default_tz)
    else:
        end_dt = end_dt.astimezone(default_tz)

    # Build attendee lists
    required_attendees = [
        Attendee(mailbox=Mailbox(email_address=email), response_type="Unknown")
        for email in event_data.get("required_attendees", [])
    ]
    optional_attendees = [
        Attendee(mailbox=Mailbox(email_address=email), response_type="Unknown")
        for email in event_data.get("optional_attendees", [])
    ]

    # Also support simple "attendees" field as required attendees
    if "attendees" in event_data and not required_attendees:
        required_attendees = [
            Attendee(mailbox=Mailbox(email_address=email), response_type="Unknown")
            for email in event_data["attendees"]
        ]

    calendar: Any = account.calendar
    item = CalendarItem(
        account=account,
        folder=calendar,
        subject=event_data["subject"],
        start=EWSDateTime.from_datetime(start_dt),
        end=EWSDateTime.from_datetime(end_dt),
        location=event_data.get("location"),
        body=event_data.get("body"),
        required_attendees=required_attendees or None,
        optional_attendees=optional_attendees or None,
    )

    # Save and send invites
    item.save(send_meeting_invitations="SendToAllAndSaveCopy")

    item_start: Any = item.start
    item_end: Any = item.end
    return {
        "id": item.id,
        "changekey": item.changekey,
        "subject": item.subject,
        "start": str(item_start.isoformat()),
        "end": str(item_end.isoformat()),
        "required_attendees": [
            a.mailbox.email_address for a in (item.required_attendees or [])
        ],
        "optional_attendees": [
            a.mailbox.email_address for a in (item.optional_attendees or [])
        ],
        "invites_sent": True,
    }


def list_invites(account: Account, limit: int = 50) -> list[dict]:
    """List pending meeting invites from inbox.

    Args:
        account: EWS account
        limit: Maximum number of results

    Returns:
        List of meeting invite dictionaries
    """
    inbox: Any = account.inbox

    # Query for MeetingRequest items
    invites = []
    for item in inbox.filter(item_class="IPM.Schedule.Meeting.Request").order_by(
        "-datetime_received"
    )[:limit]:
        if not isinstance(item, MeetingRequest):
            continue

        # Handle datetime fields
        start_str = (
            item.start.isoformat()
            if hasattr(item.start, "isoformat")
            else str(item.start)
            if item.start
            else None
        )
        end_str = (
            item.end.isoformat()
            if hasattr(item.end, "isoformat")
            else str(item.end)
            if item.end
            else None
        )

        invite = {
            "id": item.id,
            "changekey": item.changekey,
            "subject": item.subject,
            "start": start_str,
            "end": end_str,
            "location": item.location,
            "organizer": item.organizer.email_address if item.organizer else None,
            "is_all_day": item.is_all_day,
            "received": item.datetime_received.isoformat()
            if item.datetime_received
            else None,
            "response_type": item.my_response_type,
        }
        invites.append(invite)

    return invites


def rsvp_event(
    account: Account, item_id: str, response: str, message: Optional[str] = None
) -> dict:
    """Respond to a meeting invite (accept/decline/tentative).

    Args:
        account: EWS account
        item_id: The meeting request or calendar item ID
        response: One of "accept", "decline", "tentative"
        message: Optional message to include with response

    Returns:
        Dict with response status
    """
    from exchangelib import ItemId

    # Fetch item by ID using account.fetch() - EWS IDs are globally unique
    try:
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        item = items[0] if items and items[0] is not None else None
    except Exception:
        item = None

    if not item:
        return {"success": False, "error": "Meeting invite not found"}

    # Check if item supports accept/decline
    if not hasattr(item, "accept"):
        return {"success": False, "error": "Item does not support RSVP"}

    response_lower = response.lower()
    if response_lower == "accept":
        item.accept(message_disposition="SendAndSaveCopy", body=message)
    elif response_lower == "decline":
        item.decline(message_disposition="SendAndSaveCopy", body=message)
    elif response_lower in ("tentative", "maybe"):
        item.tentatively_accept(message_disposition="SendAndSaveCopy", body=message)
    else:
        return {
            "success": False,
            "error": f"Invalid response: {response}. Use accept/decline/tentative",
        }

    return {
        "success": True,
        "id": item_id,
        "response": response_lower,
        "subject": item.subject,
    }
