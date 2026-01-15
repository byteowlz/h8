"""Natural language date/time parser for calendar commands.

Supports various human-friendly formats like:
- "friday at 2pm" -> next Friday at 14:00
- "tomorrow 10:30" -> tomorrow at 10:30
- "jan 16 2pm-4pm" -> January 16 from 14:00 to 16:00
- "next monday 9am for 2h" -> next Monday 9:00, duration 2 hours
- "2026-01-16 14:00" -> ISO format passthrough
"""

import re
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Optional
from zoneinfo import ZoneInfo

from dateutil import parser as dateutil_parser
from dateutil.relativedelta import relativedelta, MO, TU, WE, TH, FR, SA, SU, weekday


# Map weekday names to dateutil constants
WEEKDAY_MAP: dict[str, weekday] = {
    "monday": MO,
    "mon": MO,
    "tuesday": TU,
    "tue": TU,
    "tues": TU,
    "wednesday": WE,
    "wed": WE,
    "thursday": TH,
    "thu": TH,
    "thur": TH,
    "thurs": TH,
    "friday": FR,
    "fri": FR,
    "saturday": SA,
    "sat": SA,
    "sunday": SU,
    "sun": SU,
}

# German weekday names
WEEKDAY_MAP_DE = {
    "montag": MO,
    "mo": MO,
    "dienstag": TU,
    "di": TU,
    "mittwoch": WE,
    "mi": WE,
    "donnerstag": TH,
    "do": TH,
    "freitag": FR,
    "fr": FR,
    "samstag": SA,
    "sa": SA,
    "sonntag": SU,
    "so": SU,
}

WEEKDAY_MAP.update(WEEKDAY_MAP_DE)

# Relative day keywords
RELATIVE_DAYS = {
    "today": 0,
    "heute": 0,
    "tomorrow": 1,
    "morgen": 1,
    "yesterday": -1,
    "gestern": -1,
    "overmorrow": 2,
    "übermorgen": 2,
}

# Duration patterns
DURATION_PATTERN = re.compile(
    r"(?:for\s+|dauer\s+)?(\d+(?:\.\d+)?)\s*(h|hr|hrs|hour|hours|stunden?|m|min|mins|minutes?|minuten?)",
    re.IGNORECASE,
)

# Time range pattern: "2pm-4pm", "14:00-16:00", "2-4pm"
# Requires at least one am/pm/uhr marker OR both sides to have :MM format
# Uses negative lookbehind to avoid matching inside dates like "2026-01-16"
TIME_RANGE_PATTERN = re.compile(
    r"(?<!\d-)(?<!\d)(\d{1,2})(?::(\d{2}))?\s*(am|pm|uhr)?\s*[-–]\s*(\d{1,2})(?::(\d{2}))?\s*(am|pm|uhr)?(?!-\d)",
    re.IGNORECASE,
)

# Single time pattern: "2pm", "14:00", "2:30pm"
# Uses word boundary and negative lookbehind to avoid matching years like "2026"
TIME_PATTERN = re.compile(
    r"(?:at\s+|um\s+)?(?<!\d)(\d{1,2})(?::(\d{2}))?\s*(am|pm|uhr)(?!\s*[-–])",
    re.IGNORECASE,
)

# 24-hour time pattern without am/pm: "14:00", "09:30"
TIME_24H_PATTERN = re.compile(
    r"(?<!\d)([01]?\d|2[0-3]):([0-5]\d)(?!\s*[-–])",
)


@dataclass
class ParsedDateTime:
    """Result of parsing a natural language date/time string."""

    start: datetime
    end: datetime
    duration_minutes: int


def _parse_time(
    hour_str: str, minute_str: Optional[str], ampm: Optional[str]
) -> tuple[int, int]:
    """Parse hour and minute from string components."""
    hour = int(hour_str)
    minute = int(minute_str) if minute_str else 0

    if ampm:
        ampm = ampm.lower()
        if ampm == "pm" and hour != 12:
            hour += 12
        elif ampm == "am" and hour == 12:
            hour = 0
        # "uhr" is 24h format, no conversion needed

    # Handle 24h format without am/pm
    if not ampm and hour <= 12:
        # Assume PM for times 1-7 without am/pm (business hours heuristic)
        if 1 <= hour <= 7:
            hour += 12

    return hour, minute


def _find_weekday(text: str) -> Optional[tuple[str, weekday]]:
    """Find a weekday name in the text and return (match, dateutil_weekday)."""
    text_lower = text.lower()

    # Check for "next <weekday>" pattern
    next_match = re.search(r"\bnext\s+(\w+)", text_lower)
    if next_match:
        day_name = next_match.group(1)
        if day_name in WEEKDAY_MAP:
            return next_match.group(0), WEEKDAY_MAP[day_name](+1)

    # Check for standalone weekday
    for name, wd in WEEKDAY_MAP.items():
        pattern = rf"\b{re.escape(name)}\b"
        if re.search(pattern, text_lower):
            return name, wd(+1)  # +1 means next occurrence

    return None


def _find_relative_day(text: str) -> Optional[tuple[str, int]]:
    """Find a relative day keyword and return (match, days_offset)."""
    text_lower = text.lower()
    for keyword, offset in RELATIVE_DAYS.items():
        pattern = rf"\b{re.escape(keyword)}\b"
        if re.search(pattern, text_lower):
            return keyword, offset
    return None


def _find_duration(text: str) -> Optional[tuple[str, int]]:
    """Find a duration specification and return (match, minutes)."""
    match = DURATION_PATTERN.search(text)
    if match:
        value = float(match.group(1))
        unit = match.group(2).lower()

        if unit.startswith("h") or unit.startswith("s"):  # hour/stunde
            minutes = int(value * 60)
        else:  # minute/minuten
            minutes = int(value)

        return match.group(0), minutes

    return None


def _find_time_range(text: str) -> Optional[tuple[str, int, int, int, int]]:
    """Find a time range and return (match, start_h, start_m, end_h, end_m)."""
    match = TIME_RANGE_PATTERN.search(text)
    if match:
        start_h, start_m = _parse_time(match.group(1), match.group(2), match.group(3))
        end_h, end_m = _parse_time(match.group(4), match.group(5), match.group(6))

        # If end time has am/pm but start doesn't, use the same period
        if match.group(6) and not match.group(3):
            end_ampm = match.group(6).lower()
            if end_ampm == "pm":
                start_test = int(match.group(1))
                if start_test < 12 and start_test < end_h - 12:
                    start_h = start_test + 12

        return match.group(0), start_h, start_m, end_h, end_m

    return None


def _find_time(text: str) -> Optional[tuple[str, int, int]]:
    """Find a single time and return (match, hour, minute)."""
    # First try 24h format (14:00)
    match = TIME_24H_PATTERN.search(text)
    if match:
        hour = int(match.group(1))
        minute = int(match.group(2))
        return match.group(0), hour, minute

    # Then try am/pm format (2pm)
    match = TIME_PATTERN.search(text)
    if match:
        hour, minute = _parse_time(match.group(1), match.group(2), match.group(3))
        return match.group(0), hour, minute
    return None


def parse_datetime(
    text: str,
    default_duration_minutes: int = 60,
    timezone: str = "Europe/Berlin",
) -> ParsedDateTime:
    """Parse a natural language date/time string.

    Args:
        text: Human-friendly date/time string like "friday at 2pm"
        default_duration_minutes: Duration to use if not specified (default: 60)
        timezone: Timezone for the resulting datetime (default: Europe/Berlin)

    Returns:
        ParsedDateTime with start, end, and duration_minutes

    Examples:
        >>> parse_datetime("friday at 2pm")
        >>> parse_datetime("tomorrow 10:30 for 2h")
        >>> parse_datetime("jan 16 2pm-4pm")
        >>> parse_datetime("next monday 9am")
    """
    tz = ZoneInfo(timezone)
    now = datetime.now(tz)

    # Start with today at a default time
    base_date = now.date()
    start_hour = 9
    start_minute = 0
    end_hour: Optional[int] = None
    end_minute: Optional[int] = None
    duration_minutes = default_duration_minutes

    remaining_text = text

    # 1. Check for relative days (today, tomorrow)
    relative = _find_relative_day(remaining_text)
    if relative:
        keyword, offset = relative
        base_date = (now + timedelta(days=offset)).date()
        remaining_text = re.sub(
            rf"\b{re.escape(keyword)}\b", "", remaining_text, flags=re.IGNORECASE
        )

    # 2. Check for weekday
    weekday_match = _find_weekday(remaining_text) if not relative else None
    if weekday_match:
        match_str, dateutil_weekday = weekday_match
        # Calculate next occurrence of this weekday
        next_day = now + relativedelta(weekday=dateutil_weekday)
        base_date = next_day.date()
        remaining_text = re.sub(
            rf"\b{re.escape(match_str)}\b", "", remaining_text, flags=re.IGNORECASE
        )

    # 3. Check for time range (2pm-4pm)
    time_range = _find_time_range(remaining_text)
    if time_range:
        match_str, start_hour, start_minute, end_hour, end_minute = time_range
        remaining_text = remaining_text.replace(match_str, "")
    else:
        # 4. Check for single time
        single_time = _find_time(remaining_text)
        if single_time:
            match_str, start_hour, start_minute = single_time
            remaining_text = remaining_text.replace(match_str, "")

    # 5. Check for duration
    duration_result = _find_duration(remaining_text)
    if duration_result:
        match_str, duration_minutes = duration_result
        remaining_text = remaining_text.replace(match_str, "")

    # 6. Try to parse any remaining date-like text with dateutil
    remaining_text = remaining_text.strip()
    remaining_text = re.sub(r"\s+", " ", remaining_text)

    # Remove common filler words
    remaining_text = re.sub(
        r"\b(at|on|um|am|für|for|next|nächste[rn]?)\b",
        "",
        remaining_text,
        flags=re.IGNORECASE,
    )
    remaining_text = remaining_text.strip()

    if remaining_text and not relative and not weekday_match:
        try:
            parsed = dateutil_parser.parse(remaining_text, fuzzy=True, dayfirst=False)
            base_date = parsed.date()
            # If dateutil extracted a time and we haven't found one yet
            if parsed.hour != 0 or parsed.minute != 0:
                if start_hour == 9 and start_minute == 0:
                    start_hour = parsed.hour
                    start_minute = parsed.minute
        except (ValueError, TypeError):
            # If parsing fails, keep the base_date we have
            pass

    # Build final datetime
    start_dt = datetime(
        base_date.year,
        base_date.month,
        base_date.day,
        start_hour,
        start_minute,
        tzinfo=tz,
    )

    # Calculate end time
    if end_hour is not None:
        end_dt = datetime(
            base_date.year,
            base_date.month,
            base_date.day,
            end_hour,
            end_minute or 0,
            tzinfo=tz,
        )
        duration_minutes = int((end_dt - start_dt).total_seconds() / 60)
    else:
        end_dt = start_dt + timedelta(minutes=duration_minutes)

    return ParsedDateTime(
        start=start_dt,
        end=end_dt,
        duration_minutes=duration_minutes,
    )


@dataclass
class ParsedDateRange:
    """Result of parsing a date range expression."""

    start: datetime
    end: datetime
    description: str  # Human-readable description like "next week", "Friday"


# Month names (English and German)
MONTH_MAP = {
    # English
    "january": 1,
    "february": 2,
    "march": 3,
    "april": 4,
    "may": 5,
    "june": 6,
    "july": 7,
    "august": 8,
    "september": 9,
    "october": 10,
    "november": 11,
    "december": 12,
    # German
    "januar": 1,
    "februar": 2,
    "märz": 3,
    "maerz": 3,
    "mai": 5,
    "juni": 6,
    "juli": 7,
    "oktober": 10,
    "dezember": 12,
    # Abbreviations
    "jan": 1,
    "feb": 2,
    "mar": 3,
    "apr": 4,
    "jun": 6,
    "jul": 7,
    "aug": 8,
    "sep": 9,
    "sept": 9,
    "oct": 10,
    "okt": 10,
    "nov": 11,
    "dec": 12,
    "dez": 12,
}


def parse_date_range(
    text: str,
    timezone: str = "Europe/Berlin",
) -> ParsedDateRange:
    """Parse a natural language date range expression for calendar viewing.

    Args:
        text: Natural language date expression like "next week", "friday", "kw30"
        timezone: Timezone for the resulting datetime (default: Europe/Berlin)

    Returns:
        ParsedDateRange with start, end, and human-readable description

    Examples:
        >>> parse_date_range("today")           # Today only
        >>> parse_date_range("tomorrow")        # Tomorrow only
        >>> parse_date_range("friday")          # Next Friday
        >>> parse_date_range("next week")       # Monday-Sunday of next week
        >>> parse_date_range("this week")       # Rest of current week
        >>> parse_date_range("kw30")            # Calendar week 30
        >>> parse_date_range("week 30")         # Calendar week 30
        >>> parse_date_range("11 dezember")     # December 11
        >>> parse_date_range("december")        # All of December
    """
    tz = ZoneInfo(timezone)
    now = datetime.now(tz)
    text_lower = text.lower().strip()

    # 1. Check for week number: "kw30", "kw 30", "week 30", "woche 30"
    week_match = re.search(
        r"\b(?:kw|week|woche)\s*(\d{1,2})\b", text_lower, re.IGNORECASE
    )
    if week_match:
        week_num = int(week_match.group(1))
        year = now.year
        # ISO week: Monday is day 1
        # Find the Monday of the given week
        jan4 = datetime(year, 1, 4, tzinfo=tz)  # Jan 4 is always in week 1
        week1_monday = jan4 - timedelta(days=jan4.weekday())
        start = week1_monday + timedelta(weeks=week_num - 1)
        end = start + timedelta(days=6, hours=23, minutes=59, seconds=59)
        return ParsedDateRange(
            start=start,
            end=end,
            description=f"KW{week_num} {year}",
        )

    # 2. Check for "next week" / "nächste woche"
    if re.search(r"\b(next\s+week|nächste\s+woche)\b", text_lower):
        # Start from next Monday
        days_until_monday = (7 - now.weekday()) % 7
        if days_until_monday == 0:
            days_until_monday = 7  # If today is Monday, go to next Monday
        next_monday = now + timedelta(days=days_until_monday)
        start = datetime(
            next_monday.year, next_monday.month, next_monday.day, 0, 0, 0, tzinfo=tz
        )
        end = start + timedelta(days=6, hours=23, minutes=59, seconds=59)
        return ParsedDateRange(start=start, end=end, description="next week")

    # 3. Check for "this week" / "diese woche"
    if re.search(r"\b(this\s+week|diese\s+woche)\b", text_lower):
        # Start from today, end on Sunday
        days_until_sunday = 6 - now.weekday()
        start = datetime(now.year, now.month, now.day, 0, 0, 0, tzinfo=tz)
        end_date = now + timedelta(days=days_until_sunday)
        end = datetime(
            end_date.year, end_date.month, end_date.day, 23, 59, 59, tzinfo=tz
        )
        return ParsedDateRange(start=start, end=end, description="this week")

    # 4. Check for relative days (today, tomorrow, etc.)
    relative = _find_relative_day(text)
    if relative:
        keyword, offset = relative
        target_date = now + timedelta(days=offset)
        start = datetime(
            target_date.year, target_date.month, target_date.day, 0, 0, 0, tzinfo=tz
        )
        end = datetime(
            target_date.year, target_date.month, target_date.day, 23, 59, 59, tzinfo=tz
        )
        return ParsedDateRange(start=start, end=end, description=keyword)

    # 5. Check for weekday (friday, montag, etc.)
    weekday_result = _find_weekday(text)
    if weekday_result:
        match_str, dateutil_weekday = weekday_result
        next_day = now + relativedelta(weekday=dateutil_weekday)
        start = datetime(
            next_day.year, next_day.month, next_day.day, 0, 0, 0, tzinfo=tz
        )
        end = datetime(
            next_day.year, next_day.month, next_day.day, 23, 59, 59, tzinfo=tz
        )
        return ParsedDateRange(start=start, end=end, description=match_str)

    # 6. Check for month name only (e.g., "december", "dezember")
    for month_name, month_num in MONTH_MAP.items():
        if re.search(rf"\b{re.escape(month_name)}\b", text_lower):
            # Check if there's also a day number
            day_match = re.search(r"\b(\d{1,2})\b", text_lower)
            if day_match:
                day = int(day_match.group(1))
                if 1 <= day <= 31:
                    year = now.year
                    # If the month is in the past, assume next year
                    if month_num < now.month or (
                        month_num == now.month and day < now.day
                    ):
                        year += 1
                    try:
                        start = datetime(year, month_num, day, 0, 0, 0, tzinfo=tz)
                        end = datetime(year, month_num, day, 23, 59, 59, tzinfo=tz)
                        return ParsedDateRange(
                            start=start,
                            end=end,
                            description=f"{day}. {month_name.capitalize()} {year}",
                        )
                    except ValueError:
                        pass  # Invalid date, continue
            else:
                # Just the month - show entire month
                year = now.year
                if month_num < now.month:
                    year += 1
                start = datetime(year, month_num, 1, 0, 0, 0, tzinfo=tz)
                # End of month
                if month_num == 12:
                    end = datetime(year + 1, 1, 1, tzinfo=tz) - timedelta(seconds=1)
                else:
                    end = datetime(year, month_num + 1, 1, tzinfo=tz) - timedelta(
                        seconds=1
                    )
                return ParsedDateRange(
                    start=start,
                    end=end,
                    description=f"{month_name.capitalize()} {year}",
                )

    # 7. Try dateutil for other formats (e.g., "11 december", "2026-01-16")
    try:
        parsed = dateutil_parser.parse(text, fuzzy=True, dayfirst=True)
        start = datetime(parsed.year, parsed.month, parsed.day, 0, 0, 0, tzinfo=tz)
        end = datetime(parsed.year, parsed.month, parsed.day, 23, 59, 59, tzinfo=tz)
        return ParsedDateRange(
            start=start,
            end=end,
            description=start.strftime("%A, %B %d, %Y"),
        )
    except (ValueError, TypeError):
        pass

    # 8. Default: show today
    start = datetime(now.year, now.month, now.day, 0, 0, 0, tzinfo=tz)
    end = datetime(now.year, now.month, now.day, 23, 59, 59, tzinfo=tz)
    return ParsedDateRange(start=start, end=end, description="today")


def parse_attendees(text: str, separator: str = "with") -> tuple[str, list[str]]:
    """Extract attendee list from text.

    Splits text on 'with' keyword and parses comma/and-separated attendee list.

    Args:
        text: Input text that may contain "with person1, person2 and person3"
        separator: Keyword that separates event info from attendees (default: "with")

    Returns:
        Tuple of (remaining_text, list_of_attendees)

    Examples:
        >>> parse_attendees("friday 2pm Team meeting with alice and bob")
        ('friday 2pm Team meeting', ['alice', 'bob'])
        >>> parse_attendees("Meeting with Roman, Alice")
        ('Meeting', ['Roman', 'Alice'])
    """
    # Look for "with" keyword (case insensitive)
    pattern = rf"\s+{re.escape(separator)}\s+(.+)$"
    match = re.search(pattern, text, re.IGNORECASE)

    if not match:
        return text, []

    remaining = text[: match.start()]
    attendee_text = match.group(1)

    # Split on comma and "and"/"und"
    attendee_text = re.sub(
        r"\s+and\s+|\s+und\s+", ",", attendee_text, flags=re.IGNORECASE
    )
    attendees = [a.strip() for a in attendee_text.split(",") if a.strip()]

    return remaining.strip(), attendees
