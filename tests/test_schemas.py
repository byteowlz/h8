"""Tests for schema converters."""

import pytest
from datetime import datetime
from zoneinfo import ZoneInfo

from h8.schemas import (
    extracted_event_to_h8,
    extracted_contact_to_h8,
    parse_time_string,
    parse_timezone,
)


class TestParseTimeString:
    """Test time string parsing."""

    def test_12h_pm(self):
        """Parse '2:00 PM'."""
        t = parse_time_string("2:00 PM")
        assert t.hour == 14
        assert t.minute == 0

    def test_12h_am(self):
        """Parse '9:30 AM'."""
        t = parse_time_string("9:30 AM")
        assert t.hour == 9
        assert t.minute == 30

    def test_24h(self):
        """Parse '14:00'."""
        t = parse_time_string("14:00")
        assert t.hour == 14
        assert t.minute == 0

    def test_24h_with_seconds(self):
        """Parse '14:00:00'."""
        t = parse_time_string("14:00:00")
        assert t.hour == 14
        assert t.minute == 0

    def test_no_space_pm(self):
        """Parse '2:00PM'."""
        t = parse_time_string("2:00PM")
        assert t.hour == 14

    def test_short_pm(self):
        """Parse '2 PM'."""
        t = parse_time_string("2 PM")
        assert t.hour == 14


class TestParseTimezone:
    """Test timezone parsing."""

    def test_iana(self):
        """Parse IANA timezone."""
        tz = parse_timezone("Europe/Berlin")
        assert str(tz) == "Europe/Berlin"

    def test_utc_offset_plus(self):
        """Parse '+01:00'."""
        tz = parse_timezone("+01:00")
        assert str(tz) == "Europe/Berlin"

    def test_utc_offset_minus(self):
        """Parse '-05:00'."""
        tz = parse_timezone("-05:00")
        assert str(tz) == "America/New_York"

    def test_abbreviation(self):
        """Parse timezone abbreviations - maps to canonical IANA zones."""
        # PST should map to America/Los_Angeles
        tz = parse_timezone("PST")
        assert str(tz) == "America/Los_Angeles"

    def test_utc(self):
        """Parse 'UTC'."""
        tz = parse_timezone("UTC")
        assert str(tz) == "UTC"

    def test_none_default(self):
        """None returns default timezone."""
        tz = parse_timezone(None)
        assert str(tz) == "Europe/Berlin"


class TestExtractedEventToH8:
    """Test event schema conversion."""

    def test_basic_event(self):
        """Convert basic event with date and time."""
        extracted = {
            "event_name": "Team Meeting",
            "date_time": {
                "start_date": "2026-01-16",
                "start_time": "2:00 PM",
                "end_time": "4:00 PM",
                "timezone": "+01:00",
            },
        }

        result = extracted_event_to_h8(extracted)

        assert result["subject"] == "Team Meeting"
        assert result["start"] == "2026-01-16T14:00:00+01:00"
        assert result["end"] == "2026-01-16T16:00:00+01:00"

    def test_event_with_location(self):
        """Convert event with physical location."""
        extracted = {
            "event_name": "Workshop",
            "date_time": {"start_date": "2026-01-20", "start_time": "9:00 AM"},
            "location": {
                "venue_name": "Building A",
                "room": "Room 101",
            },
        }

        result = extracted_event_to_h8(extracted)

        assert result["location"] == "Building A, Room 101"

    def test_event_with_virtual_link(self):
        """Convert event with virtual meeting link."""
        extracted = {
            "event_name": "Virtual Standup",
            "date_time": {"start_date": "2026-01-17", "start_time": "10:00 AM"},
            "location": {
                "type": "virtual",
                "virtual_link": "https://teams.microsoft.com/l/meetup-join/...",
            },
        }

        result = extracted_event_to_h8(extracted)

        assert "teams.microsoft.com" in result["location"]

    def test_event_with_attendees(self):
        """Convert event with attendees."""
        extracted = {
            "event_name": "Planning",
            "date_time": {"start_date": "2026-01-18", "start_time": "11:00 AM"},
            "attendees": [
                {"name": "Alice", "email": "alice@example.com"},
                {"name": "Bob", "email": "bob@example.com"},
            ],
        }

        result = extracted_event_to_h8(extracted)

        assert result["attendees"] == ["alice@example.com", "bob@example.com"]

    def test_event_with_description(self):
        """Convert event with description."""
        extracted = {
            "event_name": "Review",
            "date_time": {"start_date": "2026-01-19"},
            "description": "Q4 performance review",
        }

        result = extracted_event_to_h8(extracted)

        assert result["body"] == "Q4 performance review"

    def test_event_with_agenda(self):
        """Convert event with agenda items."""
        extracted = {
            "event_name": "Sprint Planning",
            "date_time": {"start_date": "2026-01-20", "start_time": "9:00 AM"},
            "agenda": [
                {"topic": "Review backlog", "time": "9:00"},
                {"topic": "Estimate stories", "speaker": "Alice"},
            ],
        }

        result = extracted_event_to_h8(extracted)

        assert "Agenda:" in result["body"]
        assert "Review backlog" in result["body"]
        assert "Estimate stories" in result["body"]

    def test_all_day_event(self):
        """Convert all-day event."""
        extracted = {
            "event_name": "Holiday",
            "date_time": {
                "start_date": "2026-12-25",
                "all_day": True,
            },
        }

        result = extracted_event_to_h8(extracted)

        assert "2026-12-25T00:00:00" in result["start"]
        assert "2026-12-25T23:59:59" in result["end"]

    def test_default_duration(self):
        """Event without end_time gets 1 hour default."""
        extracted = {
            "event_name": "Quick Sync",
            "date_time": {
                "start_date": "2026-01-21",
                "start_time": "3:00 PM",
            },
        }

        result = extracted_event_to_h8(extracted)

        # Start at 15:00, end at 16:00
        assert "T15:00:00" in result["start"]
        assert "T16:00:00" in result["end"]

    def test_missing_event_name_raises(self):
        """Missing event_name raises ValueError."""
        with pytest.raises(ValueError, match="event_name"):
            extracted_event_to_h8({"date_time": {"start_date": "2026-01-16"}})

    def test_missing_date_time_raises(self):
        """Missing date_time raises ValueError."""
        with pytest.raises(ValueError, match="date_time"):
            extracted_event_to_h8({"event_name": "Test"})


class TestExtractedContactToH8:
    """Test contact schema conversion."""

    def test_basic_contact(self):
        """Convert basic contact with name and email."""
        extracted = {
            "name": {
                "full_name": "Alice Smith",
                "first_name": "Alice",
                "last_name": "Smith",
            },
            "email_addresses": [{"email": "alice@example.com", "type": "work"}],
        }

        result = extracted_contact_to_h8(extracted)

        assert result["display_name"] == "Alice Smith"
        assert result["given_name"] == "Alice"
        assert result["surname"] == "Smith"
        assert result["email"] == "alice@example.com"

    def test_contact_with_phone(self):
        """Convert contact with phone numbers."""
        extracted = {
            "name": {"full_name": "Bob Jones"},
            "phone_numbers": [
                {"number": "+1-555-123-4567", "type": "work"},
                {"number": "+1-555-987-6543", "type": "mobile"},
            ],
        }

        result = extracted_contact_to_h8(extracted)

        assert result["phone"] == "+1-555-123-4567"
        assert result["mobile"] == "+1-555-987-6543"

    def test_contact_with_company(self):
        """Convert contact with company info."""
        extracted = {
            "name": {"full_name": "Charlie Brown"},
            "job_title": "Software Engineer",
            "company": {
                "name": "Acme Corp",
                "department": "Engineering",
            },
        }

        result = extracted_contact_to_h8(extracted)

        assert result["company"] == "Acme Corp"
        assert result["department"] == "Engineering"
        assert result["job_title"] == "Software Engineer"

    def test_primary_email_selected(self):
        """Primary email is selected when multiple present."""
        extracted = {
            "name": {"full_name": "Diana Prince"},
            "email_addresses": [
                {"email": "diana@work.com", "type": "work"},
                {"email": "diana@personal.com", "type": "personal", "primary": True},
            ],
        }

        result = extracted_contact_to_h8(extracted)

        assert result["email"] == "diana@personal.com"

    def test_missing_name_raises(self):
        """Missing name raises ValueError."""
        with pytest.raises(ValueError, match="name"):
            extracted_contact_to_h8(
                {"email_addresses": [{"email": "test@example.com"}]}
            )
