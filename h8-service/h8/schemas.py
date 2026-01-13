"""Schema converters for h8.

Converts between extraction schemas (used for LLM training) and h8's internal formats.
Extraction schemas are defined in byteowlz/schemas/extraction/.
"""

from datetime import datetime, time
from typing import Any, Optional
from zoneinfo import ZoneInfo

from dateutil import parser as dateutil_parser


def parse_time_string(time_str: str) -> time:
    """Parse various time formats to a time object.

    Handles: "2:00 PM", "14:00", "9:30 AM", "14:00:00"
    """
    time_str = time_str.strip()

    # Try common formats
    formats = [
        "%I:%M %p",  # 2:00 PM
        "%I:%M%p",  # 2:00PM
        "%I %p",  # 2 PM
        "%I%p",  # 2PM
        "%H:%M",  # 14:00
        "%H:%M:%S",  # 14:00:00
    ]

    for fmt in formats:
        try:
            parsed = datetime.strptime(time_str.upper(), fmt)
            return parsed.time()
        except ValueError:
            continue

    # Fallback to dateutil
    try:
        parsed = dateutil_parser.parse(time_str)
        return parsed.time()
    except (ValueError, TypeError):
        raise ValueError(f"Cannot parse time: {time_str}")


def parse_timezone(tz_str: Optional[str]) -> ZoneInfo:
    """Parse timezone string to ZoneInfo.

    Handles: "+01:00", "-05:00", "Europe/Berlin", "EST", "UTC"
    """
    if not tz_str:
        return ZoneInfo("Europe/Berlin")

    tz_str = tz_str.strip()

    # IANA timezone
    try:
        return ZoneInfo(tz_str)
    except KeyError:
        pass

    # UTC offset format (+01:00, -05:00)
    if tz_str.startswith(("+", "-")):
        # Map common offsets to IANA zones (best effort)
        offset_map = {
            "+00:00": "UTC",
            "+01:00": "Europe/Berlin",
            "+02:00": "Europe/Helsinki",
            "-05:00": "America/New_York",
            "-06:00": "America/Chicago",
            "-07:00": "America/Denver",
            "-08:00": "America/Los_Angeles",
        }
        if tz_str in offset_map:
            return ZoneInfo(offset_map[tz_str])

    # Common abbreviations
    abbrev_map = {
        "UTC": "UTC",
        "GMT": "UTC",
        "EST": "America/New_York",
        "EDT": "America/New_York",
        "CST": "America/Chicago",
        "CDT": "America/Chicago",
        "MST": "America/Denver",
        "MDT": "America/Denver",
        "PST": "America/Los_Angeles",
        "PDT": "America/Los_Angeles",
        "CET": "Europe/Berlin",
        "CEST": "Europe/Berlin",
    }
    if tz_str.upper() in abbrev_map:
        return ZoneInfo(abbrev_map[tz_str.upper()])

    # Default fallback
    return ZoneInfo("Europe/Berlin")


def extracted_event_to_h8(extracted: dict[str, Any]) -> dict[str, Any]:
    """Convert extraction/event.json format to h8 calendar event format.

    Input format (extraction/event.json):
    {
        "event_name": "Inside AI - Interview mit Thorsten Ball",
        "event_type": "meeting",
        "date_time": {
            "start_date": "2026-01-16",
            "start_time": "2:00 PM",
            "end_date": "2026-01-16",  # optional
            "end_time": "4:00 PM",     # optional
            "timezone": "+01:00",      # optional
            "all_day": false           # optional
        },
        "location": {
            "type": "virtual",
            "venue_name": "Studio B",
            "virtual_link": "https://..."
        },
        "attendees": [
            {"name": "Roman", "email": "roman@example.com"}
        ],
        "description": "..."
    }

    Output format (h8 internal):
    {
        "subject": "...",
        "start": "2026-01-16T14:00:00+01:00",
        "end": "2026-01-16T16:00:00+01:00",
        "location": "...",
        "body": "...",
        "attendees": ["email1", "email2"]
    }
    """
    if "event_name" not in extracted:
        raise ValueError("Missing required field: event_name")
    if "date_time" not in extracted:
        raise ValueError("Missing required field: date_time")

    dt = extracted["date_time"]
    if "start_date" not in dt:
        raise ValueError("Missing required field: date_time.start_date")

    # Parse timezone
    tz = parse_timezone(dt.get("timezone"))

    # Parse start datetime
    start_date = datetime.strptime(dt["start_date"], "%Y-%m-%d").date()

    if dt.get("all_day"):
        # All-day event: use midnight to midnight
        start_dt = datetime.combine(start_date, time(0, 0), tzinfo=tz)
        end_date = datetime.strptime(
            dt.get("end_date", dt["start_date"]), "%Y-%m-%d"
        ).date()
        end_dt = datetime.combine(end_date, time(23, 59, 59), tzinfo=tz)
    else:
        # Timed event
        if "start_time" in dt:
            start_time = parse_time_string(dt["start_time"])
        else:
            start_time = time(9, 0)  # Default to 9 AM

        start_dt = datetime.combine(start_date, start_time, tzinfo=tz)

        # Parse end datetime
        if "end_time" in dt:
            end_time = parse_time_string(dt["end_time"])
            end_date = datetime.strptime(
                dt.get("end_date", dt["start_date"]), "%Y-%m-%d"
            ).date()
            end_dt = datetime.combine(end_date, end_time, tzinfo=tz)
        else:
            # Default to 1 hour duration
            from datetime import timedelta

            end_dt = start_dt + timedelta(hours=1)

    # Build h8 event
    h8_event: dict[str, Any] = {
        "subject": extracted["event_name"],
        "start": start_dt.isoformat(),
        "end": end_dt.isoformat(),
    }

    # Location - combine venue_name and room, or use virtual_link
    loc = extracted.get("location", {})
    location_parts = []
    if loc.get("venue_name"):
        location_parts.append(loc["venue_name"])
    if loc.get("room"):
        location_parts.append(loc["room"])
    if loc.get("address"):
        location_parts.append(loc["address"])

    if location_parts:
        h8_event["location"] = ", ".join(location_parts)
    elif loc.get("virtual_link"):
        h8_event["location"] = loc["virtual_link"]

    # Body - combine description and agenda
    body_parts = []
    if extracted.get("description"):
        body_parts.append(extracted["description"])

    if extracted.get("agenda"):
        body_parts.append("\nAgenda:")
        for item in extracted["agenda"]:
            line = f"- {item.get('topic', '')}"
            if item.get("time"):
                line = f"{item['time']}: {item.get('topic', '')}"
            if item.get("speaker"):
                line += f" ({item['speaker']})"
            body_parts.append(line)

    if body_parts:
        h8_event["body"] = "\n".join(body_parts)

    # Attendees - extract emails
    if extracted.get("attendees"):
        attendees = []
        for att in extracted["attendees"]:
            if isinstance(att, dict) and att.get("email"):
                attendees.append(att["email"])
            elif isinstance(att, str):
                attendees.append(att)
        if attendees:
            h8_event["attendees"] = attendees

    return h8_event


def extracted_contact_to_h8(extracted: dict[str, Any]) -> dict[str, Any]:
    """Convert extraction/contact_details.json format to h8 contact format.

    Input format (extraction/contact_details.json):
    {
        "name": {
            "full_name": "Alice Smith",
            "first_name": "Alice",
            "last_name": "Smith"
        },
        "job_title": "Software Engineer",
        "company": {"name": "Acme Corp", "department": "Engineering"},
        "email_addresses": [{"email": "alice@acme.com", "type": "work"}],
        "phone_numbers": [{"number": "+1-555-123-4567", "type": "work"}]
    }

    Output format (h8 internal):
    {
        "display_name": "Alice Smith",
        "given_name": "Alice",
        "surname": "Smith",
        "email": "alice@acme.com",
        "phone": "+1-555-123-4567",
        "company": "Acme Corp",
        "job_title": "Software Engineer",
        "department": "Engineering"
    }
    """
    if "name" not in extracted:
        raise ValueError("Missing required field: name")

    name = extracted["name"]
    h8_contact: dict[str, Any] = {
        "display_name": name.get("full_name", ""),
    }

    if name.get("first_name"):
        h8_contact["given_name"] = name["first_name"]
    if name.get("last_name"):
        h8_contact["surname"] = name["last_name"]

    # Primary email
    emails = extracted.get("email_addresses", [])
    if emails:
        primary = next((e for e in emails if e.get("primary")), emails[0])
        h8_contact["email"] = primary.get("email", "")

    # Primary phone
    phones = extracted.get("phone_numbers", [])
    if phones:
        # Prefer work phone
        work = next((p for p in phones if p.get("type") == "work"), None)
        primary = work or next((p for p in phones if p.get("primary")), phones[0])
        h8_contact["phone"] = primary.get("number", "")

        # Mobile as separate field
        mobile = next((p for p in phones if p.get("type") == "mobile"), None)
        if mobile:
            h8_contact["mobile"] = mobile["number"]

    # Company info
    company = extracted.get("company", {})
    if company.get("name"):
        h8_contact["company"] = company["name"]
    if company.get("department"):
        h8_contact["department"] = company["department"]

    if extracted.get("job_title"):
        h8_contact["job_title"] = extracted["job_title"]

    if extracted.get("notes"):
        h8_contact["notes"] = extracted["notes"]

    return h8_contact
