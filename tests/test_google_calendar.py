"""Tests for the Google Calendar backend mixin.

Everything is exercised against fully stubbed Calendar v3 service objects -- no
network, no discovery, no ``googleapiclient`` import. Fakes record the request
kwargs/bodies so we can assert on ``sendUpdates``, freebusy query items, RSVP
patch bodies, etc., and return canned API responses.
"""

from __future__ import annotations

from datetime import datetime
from types import SimpleNamespace
from zoneinfo import ZoneInfo

import pytest

from h8.providers.base import CAP_RESOURCES, BackendError, BackendNotSupported
from h8.providers.google import calendar as gcal
from h8.providers.google.calendar import (
    GoogleCalendarMixin,
    _busy_intervals_from_freebusy,
    _find_slots_from_busy_times,
    _merge_busy_times,
)

BERLIN = ZoneInfo("Europe/Berlin")

# Golden key set for the list/search shape, mirroring the dict built in
# ``h8/calendar.py::list_events`` (``meeting_url`` is additive/optional there).
GOLDEN_LIST_KEYS = {
    "id",
    "changekey",
    "subject",
    "start",
    "end",
    "location",
    "organizer",
    "is_all_day",
    "is_cancelled",
}


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class _Exec:
    def __init__(self, result):
        self._result = result

    def execute(self):
        return self._result


class _FakeEventsApi:
    def __init__(self, rec):
        self._rec = rec

    def list(self, **kwargs):
        self._rec.list_calls.append(kwargs)
        if self._rec.list_queue:
            resp = self._rec.list_queue.pop(0)
        else:
            resp = {"items": []}
        return _Exec(resp)

    def get(self, **kwargs):
        self._rec.get_calls.append(kwargs)
        if self._rec.get_raises is not None:
            raise self._rec.get_raises
        return _Exec(self._rec.get_response)

    def insert(self, **kwargs):
        self._rec.insert_calls.append(kwargs)
        return _Exec(self._rec.insert_response)

    def delete(self, **kwargs):
        self._rec.delete_calls.append(kwargs)
        if self._rec.delete_raises is not None:
            raise self._rec.delete_raises
        return _Exec({})

    def patch(self, **kwargs):
        self._rec.patch_calls.append(kwargs)
        return _Exec(self._rec.get_response)


class _FakeFreebusyApi:
    def __init__(self, rec):
        self._rec = rec

    def query(self, body):
        self._rec.freebusy_calls.append(body)
        return _Exec(self._rec.freebusy_response)


class _FakeService:
    def __init__(self, rec):
        self._rec = rec

    def events(self):
        return _FakeEventsApi(self._rec)

    def freebusy(self):
        return _FakeFreebusyApi(self._rec)


class _Recorder:
    def __init__(self):
        self.list_calls = []
        self.get_calls = []
        self.insert_calls = []
        self.delete_calls = []
        self.patch_calls = []
        self.freebusy_calls = []

        self.list_queue = []
        self.get_response = None
        self.get_raises = None
        self.insert_response = None
        self.delete_raises = None
        self.freebusy_response = {"calendars": {}}


class _StubClient:
    def __init__(self, service):
        self._service = service

    def calendar(self):
        return self._service


def make_mixin(rec, email="me@example.com"):
    mixin = GoogleCalendarMixin()
    mixin._client = _StubClient(_FakeService(rec))
    mixin.account = SimpleNamespace(email=email)
    return mixin


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(autouse=True)
def _fixed_config(monkeypatch):
    """Pin timezone/working-hours so tests are deterministic regardless of env."""
    config = {
        "timezone": "Europe/Berlin",
        "free_slots": {"start_hour": 9, "end_hour": 17, "exclude_weekends": True},
    }
    monkeypatch.setattr(gcal, "get_config", lambda: config)
    return config


# ---------------------------------------------------------------------------
# Sample events
# ---------------------------------------------------------------------------


def _timed_event():
    return {
        "id": "evt-timed",
        "summary": "Sync",
        "location": "Room A",
        "status": "confirmed",
        "organizer": {"email": "boss@example.com", "displayName": "Boss"},
        "start": {"dateTime": "2026-07-10T14:00:00Z"},
        "end": {"dateTime": "2026-07-10T15:00:00Z"},
    }


def _all_day_event():
    return {
        "id": "evt-allday",
        "summary": "Holiday",
        "status": "confirmed",
        "organizer": {"email": "me@example.com"},
        "start": {"date": "2026-07-10"},
        "end": {"date": "2026-07-11"},
    }


# ---------------------------------------------------------------------------
# list_events / shape parity
# ---------------------------------------------------------------------------


def test_list_events_golden_key_parity():
    rec = _Recorder()
    rec.list_queue = [{"items": [_timed_event()]}]
    mixin = make_mixin(rec)

    events = mixin.list_events(days=7)

    assert len(events) == 1
    assert set(events[0].keys()) == GOLDEN_LIST_KEYS

    # View request shape mirrors EWS calendar-view recurrence expansion.
    call = rec.list_calls[0]
    assert call["calendarId"] == "primary"
    assert call["singleEvents"] is True
    assert call["orderBy"] == "startTime"
    assert "timeMin" in call and "timeMax" in call


def test_list_events_includes_meeting_url_only_when_present():
    rec = _Recorder()
    online = _timed_event()
    online["hangoutLink"] = "https://meet.google.com/abc-defg-hij"
    rec.list_queue = [{"items": [online]}]
    mixin = make_mixin(rec)

    event = mixin.list_events()[0]
    assert event["meeting_url"] == "https://meet.google.com/abc-defg-hij"
    assert set(event.keys()) == GOLDEN_LIST_KEYS | {"meeting_url"}


def test_list_events_timed_mapping():
    rec = _Recorder()
    rec.list_queue = [{"items": [_timed_event()]}]
    mixin = make_mixin(rec)

    event = mixin.list_events()[0]
    assert event["is_all_day"] is False
    assert event["is_cancelled"] is False
    assert event["organizer"] == "boss@example.com"
    assert event["changekey"] is None
    # 14:00Z -> 16:00 Berlin (+02:00 in July), ISO-8601 with offset.
    assert event["start"] == "2026-07-10T16:00:00+02:00"
    assert event["end"] == "2026-07-10T17:00:00+02:00"


def test_list_events_all_day_mapping():
    rec = _Recorder()
    rec.list_queue = [{"items": [_all_day_event()]}]
    mixin = make_mixin(rec)

    event = mixin.list_events()[0]
    assert event["is_all_day"] is True
    assert event["start"] == "2026-07-10"
    assert event["end"] == "2026-07-11"


def test_cancelled_status_maps_to_is_cancelled():
    rec = _Recorder()
    ev = _timed_event()
    ev["status"] = "cancelled"
    rec.list_queue = [{"items": [ev]}]
    mixin = make_mixin(rec)
    assert mixin.list_events()[0]["is_cancelled"] is True


# ---------------------------------------------------------------------------
# get_event detail
# ---------------------------------------------------------------------------


def test_get_event_detail_shape():
    rec = _Recorder()
    ev = _timed_event()
    ev["description"] = "agenda"
    ev["attendees"] = [
        {"email": "me@example.com", "self": True, "responseStatus": "accepted"},
        {
            "email": "opt@example.com",
            "optional": True,
            "responseStatus": "needsAction",
            "displayName": "Opt",
        },
    ]
    rec.get_response = ev
    mixin = make_mixin(rec)

    detail = mixin.get_event("evt-timed")
    assert detail["success"] is True
    assert detail["changekey"] is None
    assert detail["organizer_name"] == "Boss"
    assert detail["my_response"] == "accepted"
    assert detail["required_attendees"] == [
        {"email": "me@example.com", "name": None, "response": "accepted"}
    ]
    assert detail["optional_attendees"] == [
        {"email": "opt@example.com", "name": "Opt", "response": "needsAction"}
    ]
    assert detail["sensitivity"] == "Normal"
    assert detail["importance"] == "Normal"
    assert detail["body"] == "agenda"


def test_get_event_not_found_returns_error_dict():
    rec = _Recorder()
    rec.get_raises = RuntimeError("404")
    mixin = make_mixin(rec)
    result = mixin.get_event("missing")
    assert result["success"] is False
    assert "Failed to get event" in result["error"]


def test_get_event_recurrence_best_effort():
    rec = _Recorder()
    ev = _timed_event()
    ev["recurrence"] = ["RRULE:FREQ=WEEKLY;UNTIL=20261231T000000Z"]
    rec.get_response = ev
    mixin = make_mixin(rec)
    detail = mixin.get_event("evt-timed")
    assert detail["recurrence"] == {"pattern": "WEEKLY", "range": "end_date"}


# ---------------------------------------------------------------------------
# create / invite
# ---------------------------------------------------------------------------


def test_create_event_no_attendees_sends_none():
    rec = _Recorder()
    rec.insert_response = {
        "id": "new-1",
        "summary": "Focus",
        "start": {"dateTime": "2026-07-10T09:00:00+02:00"},
        "end": {"dateTime": "2026-07-10T10:00:00+02:00"},
    }
    mixin = make_mixin(rec)

    result = mixin.create_event(
        {"subject": "Focus", "start": "2026-07-10T09:00:00", "end": "2026-07-10T10:00:00"}
    )
    call = rec.insert_calls[0]
    assert call["sendUpdates"] == "none"
    assert "attendees" not in call["body"]
    assert call["body"]["start"]["timeZone"] == "Europe/Berlin"
    assert result["id"] == "new-1"
    assert result["changekey"] is None
    assert set(result.keys()) == {"id", "changekey", "subject", "start", "end"}


def test_create_event_with_attendees_sets_send_updates_all():
    rec = _Recorder()
    rec.insert_response = {
        "id": "new-2",
        "summary": "Sync",
        "start": {"dateTime": "2026-07-10T09:00:00+02:00"},
        "end": {"dateTime": "2026-07-10T10:00:00+02:00"},
    }
    mixin = make_mixin(rec)

    mixin.create_event(
        {
            "subject": "Sync",
            "start": "2026-07-10T09:00:00",
            "end": "2026-07-10T10:00:00",
            "attendees": ["a@example.com", "b@example.com"],
        }
    )
    call = rec.insert_calls[0]
    assert call["sendUpdates"] == "all"
    assert call["body"]["attendees"] == [
        {"email": "a@example.com"},
        {"email": "b@example.com"},
    ]


def test_create_event_all_day_uses_date_fields():
    rec = _Recorder()
    rec.insert_response = {
        "id": "new-3",
        "summary": "PTO",
        "start": {"date": "2026-07-10"},
        "end": {"date": "2026-07-11"},
    }
    mixin = make_mixin(rec)

    mixin.create_event(
        {
            "subject": "PTO",
            "start": "2026-07-10",
            "end": "2026-07-10",
            "is_all_day": True,
        }
    )
    body = rec.insert_calls[0]["body"]
    assert body["start"] == {"date": "2026-07-10"}
    # End date is made exclusive when it collapses onto the start date.
    assert body["end"] == {"date": "2026-07-11"}
    assert rec.insert_calls[0]["sendUpdates"] == "none"


def test_invite_event_sends_updates_and_echoes_attendees():
    rec = _Recorder()
    rec.insert_response = {
        "id": "inv-1",
        "summary": "Kickoff",
        "start": {"dateTime": "2026-07-10T09:00:00+02:00"},
        "end": {"dateTime": "2026-07-10T10:00:00+02:00"},
    }
    mixin = make_mixin(rec)

    result = mixin.invite_event(
        {
            "subject": "Kickoff",
            "start": "2026-07-10T09:00:00",
            "end": "2026-07-10T10:00:00",
            "required_attendees": ["req@example.com"],
            "optional_attendees": ["opt@example.com"],
        }
    )
    call = rec.insert_calls[0]
    assert call["sendUpdates"] == "all"
    assert call["body"]["attendees"] == [
        {"email": "req@example.com"},
        {"email": "opt@example.com", "optional": True},
    ]
    assert result["required_attendees"] == ["req@example.com"]
    assert result["optional_attendees"] == ["opt@example.com"]
    assert result["invites_sent"] is True


# ---------------------------------------------------------------------------
# delete / cancel
# ---------------------------------------------------------------------------


def test_delete_event_does_not_notify():
    rec = _Recorder()
    mixin = make_mixin(rec)
    result = mixin.delete_event("evt-1")
    assert result == {"success": True, "id": "evt-1"}
    assert rec.delete_calls[0]["sendUpdates"] == "none"


def test_cancel_event_notifies_all():
    rec = _Recorder()
    rec.get_response = {
        "id": "evt-1",
        "summary": "Standup",
        "organizer": {"email": "me@example.com", "self": True},
    }
    mixin = make_mixin(rec)
    result = mixin.cancel_event("evt-1", message="sorry")
    assert result["success"] is True
    assert result["cancellation_sent"] is True
    assert result["subject"] == "Standup"
    assert result["message"] == "sorry"
    assert rec.delete_calls[0]["sendUpdates"] == "all"


def test_cancel_event_rejects_non_organizer():
    rec = _Recorder()
    rec.get_response = {
        "id": "evt-1",
        "summary": "Standup",
        "organizer": {"email": "someone-else@example.com"},
    }
    mixin = make_mixin(rec)
    result = mixin.cancel_event("evt-1")
    assert result["success"] is False
    assert "not the organizer" in result["error"]
    assert rec.delete_calls == []


# ---------------------------------------------------------------------------
# search / invites
# ---------------------------------------------------------------------------


def test_search_events_passes_query_and_limit():
    rec = _Recorder()
    rec.list_queue = [{"items": [_timed_event(), _all_day_event()]}]
    mixin = make_mixin(rec)
    results = mixin.search_events("Sync", limit=1)
    assert len(results) == 1
    assert rec.list_calls[0]["q"] == "Sync"


def test_list_invites_filters_needs_action():
    rec = _Recorder()
    accepted = _timed_event()
    accepted["id"] = "acc"
    accepted["attendees"] = [
        {"email": "me@example.com", "self": True, "responseStatus": "accepted"}
    ]
    pending = _timed_event()
    pending["id"] = "pend"
    pending["created"] = "2026-07-01T10:00:00Z"
    pending["attendees"] = [
        {"email": "me@example.com", "self": True, "responseStatus": "needsAction"}
    ]
    rec.list_queue = [{"items": [accepted, pending]}]
    mixin = make_mixin(rec)

    invites = mixin.list_invites()
    assert [i["id"] for i in invites] == ["pend"]
    assert invites[0]["response_type"] == "needsAction"
    assert invites[0]["received"] == "2026-07-01T10:00:00Z"


# ---------------------------------------------------------------------------
# rsvp
# ---------------------------------------------------------------------------


def _event_with_self_attendee():
    return {
        "id": "evt-rsvp",
        "summary": "Review",
        "attendees": [
            {"email": "other@example.com", "responseStatus": "accepted"},
            {"email": "me@example.com", "self": True, "responseStatus": "needsAction"},
        ],
    }


@pytest.mark.parametrize(
    "response,expected_status,expected_norm",
    [
        ("accept", "accepted", "accept"),
        ("decline", "declined", "decline"),
        ("tentative", "tentative", "tentative"),
        ("maybe", "tentative", "tentative"),
    ],
)
def test_rsvp_patch_body(response, expected_status, expected_norm):
    rec = _Recorder()
    rec.get_response = _event_with_self_attendee()
    mixin = make_mixin(rec)

    result = mixin.rsvp_event("evt-rsvp", response)

    patch = rec.patch_calls[0]
    assert patch["calendarId"] == "primary"
    assert patch["eventId"] == "evt-rsvp"
    assert patch["sendUpdates"] == "all"
    patched = {a["email"]: a["responseStatus"] for a in patch["body"]["attendees"]}
    assert patched["me@example.com"] == expected_status
    # Other attendees are preserved untouched.
    assert patched["other@example.com"] == "accepted"

    assert result["success"] is True
    assert result["response"] == expected_norm
    assert result["subject"] == "Review"


def test_rsvp_invalid_response():
    rec = _Recorder()
    mixin = make_mixin(rec)
    result = mixin.rsvp_event("evt", "bogus")
    assert result["success"] is False
    assert rec.get_calls == []


def test_rsvp_no_self_attendee():
    rec = _Recorder()
    rec.get_response = {
        "id": "evt",
        "summary": "x",
        "attendees": [{"email": "other@example.com", "responseStatus": "accepted"}],
    }
    mixin = make_mixin(rec)
    result = mixin.rsvp_event("evt", "accept")
    assert result["success"] is False
    assert rec.patch_calls == []


# ---------------------------------------------------------------------------
# freebusy -> slot computation
# ---------------------------------------------------------------------------


def _freebusy(cal_id, busy_blocks):
    return {"calendars": {cal_id: {"busy": busy_blocks}}}


def test_busy_intervals_parse_and_convert_to_config_tz():
    resp = _freebusy(
        "primary",
        [{"start": "2026-07-06T08:00:00Z", "end": "2026-07-06T09:00:00Z"}],
    )
    intervals = _busy_intervals_from_freebusy(resp, BERLIN, raise_on_error=False)
    assert len(intervals) == 1
    start, end = intervals[0]
    assert start == datetime(2026, 7, 6, 10, 0, tzinfo=BERLIN)
    assert end == datetime(2026, 7, 6, 11, 0, tzinfo=BERLIN)


def test_slot_computation_no_busy_full_working_day():
    now = datetime(2026, 7, 6, 8, 0, tzinfo=BERLIN)  # Monday, before work start
    end_date = datetime(2026, 7, 6, 23, 59, 59, tzinfo=BERLIN)
    slots = _find_slots_from_busy_times(
        [], now, end_date, BERLIN, 9, 17, True, 30, None
    )
    assert len(slots) == 1
    assert slots[0]["start"] == "2026-07-06T09:00:00+02:00"
    assert slots[0]["end"] == "2026-07-06T17:00:00+02:00"
    assert slots[0]["duration_minutes"] == 480
    assert slots[0]["day"] == "Monday"


def test_slot_computation_with_busy_block_from_freebusy():
    now = datetime(2026, 7, 6, 8, 0, tzinfo=BERLIN)
    end_date = datetime(2026, 7, 6, 23, 59, 59, tzinfo=BERLIN)
    # Busy 10:00-11:00 Berlin (08:00-09:00 UTC) sourced from a freebusy reply.
    resp = _freebusy(
        "primary",
        [{"start": "2026-07-06T08:00:00Z", "end": "2026-07-06T09:00:00Z"}],
    )
    busy = _busy_intervals_from_freebusy(resp, BERLIN, raise_on_error=False)
    merged = _merge_busy_times(sorted(busy))
    slots = _find_slots_from_busy_times(
        merged, now, end_date, BERLIN, 9, 17, True, 30, None
    )
    windows = [(s["start"][11:16], s["end"][11:16]) for s in slots]
    assert windows == [("09:00", "10:00"), ("11:00", "17:00")]


def test_slot_computation_excludes_weekends():
    now = datetime(2026, 7, 11, 8, 0, tzinfo=BERLIN)  # Saturday
    end_date = datetime(2026, 7, 11, 23, 59, 59, tzinfo=BERLIN)
    slots = _find_slots_from_busy_times(
        [], now, end_date, BERLIN, 9, 17, True, 30, None
    )
    assert slots == []


def test_find_free_slots_queries_primary():
    rec = _Recorder()
    rec.freebusy_response = _freebusy("primary", [])
    mixin = make_mixin(rec)
    slots = mixin.find_free_slots(weeks=1, duration_minutes=30)
    assert isinstance(slots, list)
    body = rec.freebusy_calls[0]
    assert body["items"] == [{"id": "primary"}]
    assert body["timeZone"] == "Europe/Berlin"


# ---------------------------------------------------------------------------
# other people
# ---------------------------------------------------------------------------


def test_get_person_agenda_pseudo_events():
    rec = _Recorder()
    rec.freebusy_response = _freebusy(
        "friend@example.com",
        [{"start": "2026-07-06T08:00:00Z", "end": "2026-07-06T09:00:00Z"}],
    )
    mixin = make_mixin(rec)

    agenda = mixin.get_person_agenda("friend@example.com", days=3)
    assert agenda == [
        {
            "start": "2026-07-06T10:00:00+02:00",
            "end": "2026-07-06T11:00:00+02:00",
            "status": "Busy",
            "subject": "Busy",
        }
    ]
    assert rec.freebusy_calls[0]["items"] == [{"id": "friend@example.com"}]


def test_get_person_free_slots_queries_email():
    rec = _Recorder()
    rec.freebusy_response = _freebusy("friend@example.com", [])
    mixin = make_mixin(rec)
    mixin.get_person_free_slots("friend@example.com")
    assert rec.freebusy_calls[0]["items"] == [{"id": "friend@example.com"}]


def test_get_person_free_slots_permission_error():
    rec = _Recorder()
    rec.freebusy_response = {
        "calendars": {
            "friend@example.com": {"errors": [{"reason": "notFound"}], "busy": []}
        }
    }
    mixin = make_mixin(rec)
    with pytest.raises(BackendError) as exc:
        mixin.get_person_free_slots("friend@example.com")
    assert "domain" in str(exc.value).lower()


def test_find_common_free_slots_queries_all_emails():
    rec = _Recorder()
    rec.freebusy_response = {
        "calendars": {"a@example.com": {"busy": []}, "b@example.com": {"busy": []}}
    }
    mixin = make_mixin(rec)
    mixin.find_common_free_slots(["a@example.com", "b@example.com"])
    assert rec.freebusy_calls[0]["items"] == [
        {"id": "a@example.com"},
        {"id": "b@example.com"},
    ]


# ---------------------------------------------------------------------------
# resources (unsupported)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("method", ["resource_free", "resource_free_window", "resource_agenda"])
def test_resource_methods_not_supported(method):
    rec = _Recorder()
    mixin = make_mixin(rec)
    args = {
        "resource_free": ([],),
        "resource_free_window": ([], "2026-07-06", "2026-07-07"),
        "resource_agenda": ([],),
    }[method]
    with pytest.raises(BackendNotSupported) as exc:
        getattr(mixin, method)(*args)
    assert exc.value.capability == CAP_RESOURCES
