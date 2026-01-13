"""Tests for the natural language date/time parser."""

import pytest
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

from h8.dateparser import (
    parse_datetime,
    parse_attendees,
    ParsedDateTime,
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
