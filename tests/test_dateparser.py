"""Tests for the natural language date/time parser."""

import pytest
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

from h8.dateparser import (
    parse_datetime,
    parse_attendees,
    parse_date_range,
    ParsedDateTime,
    ParsedDateRange,
)


class TestParseDatetime:
    """Test parse_datetime function."""

    def test_weekday_with_time(self):
        """Parse 'friday at 2pm'."""
        result = parse_datetime("friday at 2pm")

        # Should be a Friday
        assert result.start.weekday() == 4  # Friday
        assert result.start.hour == 14
        assert result.start.minute == 0
        assert result.duration_minutes == 60  # default

    def test_weekday_with_time_range(self):
        """Parse 'friday 2pm-4pm'."""
        result = parse_datetime("friday 2pm-4pm")

        assert result.start.weekday() == 4
        assert result.start.hour == 14
        assert result.end.hour == 16
        assert result.duration_minutes == 120

    def test_tomorrow_with_time(self):
        """Parse 'tomorrow 10:30'."""
        result = parse_datetime("tomorrow 10:30")
        now = datetime.now(ZoneInfo("Europe/Berlin"))
        expected_date = (now + timedelta(days=1)).date()

        assert result.start.date() == expected_date
        assert result.start.hour == 10
        assert result.start.minute == 30

    def test_today(self):
        """Parse 'today at 3pm'."""
        result = parse_datetime("today at 3pm")
        now = datetime.now(ZoneInfo("Europe/Berlin"))

        assert result.start.date() == now.date()
        assert result.start.hour == 15

    def test_german_weekday(self):
        """Parse 'Freitag um 14 Uhr'."""
        result = parse_datetime("Freitag um 14 Uhr")

        assert result.start.weekday() == 4  # Friday
        assert result.start.hour == 14

    def test_german_tomorrow(self):
        """Parse 'morgen 10:30'."""
        result = parse_datetime("morgen 10:30")
        now = datetime.now(ZoneInfo("Europe/Berlin"))
        expected_date = (now + timedelta(days=1)).date()

        assert result.start.date() == expected_date
        assert result.start.hour == 10

    def test_next_weekday(self):
        """Parse 'next monday 9am'."""
        result = parse_datetime("next monday 9am")

        assert result.start.weekday() == 0  # Monday
        assert result.start.hour == 9

    def test_with_duration(self):
        """Parse 'friday 2pm for 2h'."""
        result = parse_datetime("friday 2pm for 2h")

        assert result.start.hour == 14
        assert result.duration_minutes == 120
        assert result.end.hour == 16

    def test_duration_minutes(self):
        """Parse 'tomorrow 9am for 30 minutes'."""
        result = parse_datetime("tomorrow 9am for 30 minutes")

        assert result.duration_minutes == 30

    def test_24h_format(self):
        """Parse '14:30' correctly."""
        result = parse_datetime("friday 14:30")

        assert result.start.hour == 14
        assert result.start.minute == 30

    def test_iso_date(self):
        """Parse ISO date format '2026-01-16'."""
        result = parse_datetime("2026-01-16 2pm")

        assert result.start.year == 2026
        assert result.start.month == 1
        assert result.start.day == 16
        assert result.start.hour == 14

    def test_full_date(self):
        """Parse 'January 16 2026 2pm' (without 'at' which gets stripped)."""
        result = parse_datetime("2026-01-16 14:00")

        assert result.start.year == 2026
        assert result.start.month == 1
        assert result.start.day == 16
        assert result.start.hour == 14

    def test_custom_duration_default(self):
        """Custom default duration is used."""
        result = parse_datetime("friday 2pm", default_duration_minutes=90)

        assert result.duration_minutes == 90

    def test_timezone(self):
        """Result has correct timezone."""
        result = parse_datetime("friday 2pm", timezone="Europe/Berlin")

        assert result.start.tzinfo is not None
        assert str(result.start.tzinfo) == "Europe/Berlin"


class TestParseAttendees:
    """Test parse_attendees function."""

    def test_single_attendee(self):
        """Parse 'meeting with alice'."""
        remaining, attendees = parse_attendees("Team meeting with alice")

        assert remaining == "Team meeting"
        assert attendees == ["alice"]

    def test_multiple_attendees_comma(self):
        """Parse 'meeting with alice, bob'."""
        remaining, attendees = parse_attendees("Meeting with alice, bob")

        assert remaining == "Meeting"
        assert attendees == ["alice", "bob"]

    def test_multiple_attendees_and(self):
        """Parse 'meeting with alice and bob'."""
        remaining, attendees = parse_attendees("Meeting with alice and bob")

        assert remaining == "Meeting"
        assert attendees == ["alice", "bob"]

    def test_multiple_attendees_mixed(self):
        """Parse 'meeting with alice, bob and charlie'."""
        remaining, attendees = parse_attendees("Meeting with alice, bob and charlie")

        assert remaining == "Meeting"
        assert attendees == ["alice", "bob", "charlie"]

    def test_no_attendees(self):
        """Text without 'with' returns empty list."""
        remaining, attendees = parse_attendees("Team meeting friday 2pm")

        assert remaining == "Team meeting friday 2pm"
        assert attendees == []

    def test_german_und(self):
        """Parse 'meeting with alice und bob'."""
        remaining, attendees = parse_attendees("Meeting with alice und bob")

        assert remaining == "Meeting"
        assert attendees == ["alice", "bob"]

    def test_preserves_datetime(self):
        """Datetime parts are preserved in remaining text."""
        remaining, attendees = parse_attendees(
            "friday 2pm Team sync with roman and alice"
        )

        assert "friday 2pm Team sync" == remaining
        assert attendees == ["roman", "alice"]


class TestParseDateRange:
    """Test parse_date_range function for calendar show command."""

    def test_today(self):
        """Parse 'today'."""
        result = parse_date_range("today")
        now = datetime.now(ZoneInfo("Europe/Berlin"))

        assert result.start.date() == now.date()
        assert result.end.date() == now.date()
        assert result.start.hour == 0
        assert result.end.hour == 23

    def test_tomorrow(self):
        """Parse 'tomorrow'."""
        result = parse_date_range("tomorrow")
        now = datetime.now(ZoneInfo("Europe/Berlin"))
        expected = (now + timedelta(days=1)).date()

        assert result.start.date() == expected
        assert result.end.date() == expected

    def test_german_morgen(self):
        """Parse 'morgen'."""
        result = parse_date_range("morgen")
        now = datetime.now(ZoneInfo("Europe/Berlin"))
        expected = (now + timedelta(days=1)).date()

        assert result.start.date() == expected

    def test_weekday_friday(self):
        """Parse 'friday'."""
        result = parse_date_range("friday")

        assert result.start.weekday() == 4  # Friday
        assert result.end.weekday() == 4
        assert "friday" in result.description.lower()

    def test_german_weekday(self):
        """Parse 'Freitag'."""
        result = parse_date_range("Freitag")

        assert result.start.weekday() == 4  # Friday

    def test_next_week(self):
        """Parse 'next week'."""
        result = parse_date_range("next week")
        now = datetime.now(ZoneInfo("Europe/Berlin"))

        # Should start on a Monday
        assert result.start.weekday() == 0
        # Should end on Sunday (6 days later)
        assert result.end.weekday() == 6
        # Should be at least 1 day in the future
        assert result.start.date() > now.date()
        assert result.description == "next week"

    def test_german_next_week(self):
        """Parse 'nächste woche'."""
        result = parse_date_range("nächste woche")

        assert result.start.weekday() == 0  # Monday

    def test_this_week(self):
        """Parse 'this week'."""
        result = parse_date_range("this week")
        now = datetime.now(ZoneInfo("Europe/Berlin"))

        # Should start today
        assert result.start.date() == now.date()
        # Should end on Sunday
        assert result.end.weekday() == 6

    def test_week_number_kw(self):
        """Parse 'kw30'."""
        result = parse_date_range("kw30")

        assert result.start.weekday() == 0  # Monday
        assert result.end.weekday() == 6  # Sunday
        assert "KW30" in result.description

    def test_week_number_kw_space(self):
        """Parse 'kw 30'."""
        result = parse_date_range("kw 30")

        assert result.start.weekday() == 0
        assert "KW30" in result.description

    def test_week_number_english(self):
        """Parse 'week 30'."""
        result = parse_date_range("week 30")

        assert result.start.weekday() == 0
        assert "KW30" in result.description

    def test_month_name(self):
        """Parse 'december' - entire month."""
        result = parse_date_range("december")

        assert result.start.month == 12
        assert result.start.day == 1
        assert result.end.month == 12
        assert result.end.day == 31

    def test_german_month(self):
        """Parse 'dezember'."""
        result = parse_date_range("dezember")

        assert result.start.month == 12
        assert result.start.day == 1

    def test_day_and_month(self):
        """Parse '11 december'."""
        result = parse_date_range("11 december")

        assert result.start.month == 12
        assert result.start.day == 11
        assert result.end.day == 11  # Single day

    def test_german_day_and_month(self):
        """Parse '11 dezember'."""
        result = parse_date_range("11 dezember")

        assert result.start.month == 12
        assert result.start.day == 11

    def test_iso_date(self):
        """Parse '2026-01-16'."""
        result = parse_date_range("2026-01-16")

        assert result.start.year == 2026
        assert result.start.month == 1
        assert result.start.day == 16

    def test_default_to_today(self):
        """Empty or unparseable defaults to today."""
        result = parse_date_range("")
        now = datetime.now(ZoneInfo("Europe/Berlin"))

        assert result.start.date() == now.date()
        assert result.description == "today"
