"""Tests for the Google contacts and settings (OOF) backend mixins.

Exercises :class:`~h8.providers.google.contacts.GoogleContactsMixin` and
:class:`~h8.providers.google.settings.GoogleSettingsMixin` against stubbed
People API / Gmail service objects -- no network, no real googleapiclient
``build()`` call. Covers:

- ``list_contacts``/``get_contact`` dict-shape parity with ``h8.contacts``.
- ``update_contact`` sends the correct ``updatePersonFields`` mask and etag.
- OOF get/set mapping in both directions (enabled/scheduled/disabled).
- Inbox rule methods raise ``BackendNotSupported``.
"""

from __future__ import annotations

from typing import Any, Callable, Optional
from unittest.mock import MagicMock

import pytest

from h8.providers.base import AccountConfig, BackendNotSupported
from h8.providers.google.contacts import GoogleContactsMixin
from h8.providers.google.settings import GoogleSettingsMixin


# ---------------------------------------------------------------------------
# Stub plumbing
# ---------------------------------------------------------------------------


class _FakeRequest:
    """Mimics a googleapiclient ``HttpRequest``: ``.execute()`` returns a canned value."""

    def __init__(self, result: Any = None, recorder: Optional[Callable[..., None]] = None) -> None:
        self._result = result

    def execute(self) -> Any:
        return self._result


class _RaisingRequest:
    """A fake request whose ``.execute()`` raises, mirroring real googleapiclient
    behavior where the HTTP error surfaces from ``execute()``, not the resource
    method call itself."""

    def __init__(self, exc: BaseException) -> None:
        self._exc = exc

    def execute(self) -> Any:
        raise self._exc


class FakePeopleResource:
    """Stands in for ``people_service.people()``.

    Records the kwargs of the last call to each method so tests can assert on
    them, and returns pre-programmed responses.
    """

    def __init__(self) -> None:
        self.connections_list_calls: list[dict] = []
        self.get_calls: list[dict] = []
        self.create_calls: list[dict] = []
        self.update_calls: list[dict] = []
        self.delete_calls: list[dict] = []

        # Maps a request's `pageToken` (None for the first page) to the
        # response for that page, so pagination chains via `nextPageToken`
        # regardless of how many separate top-level calls a test makes.
        self.connections_pages: dict[Optional[str], dict] = {None: {"connections": []}}
        self.get_response: dict = {}
        self.create_response: dict = {}
        self.update_response: dict = {}
        self.delete_response: dict = {}

    def connections(self) -> "FakePeopleResource":
        return self

    def list(self, **kwargs: Any) -> _FakeRequest:
        self.connections_list_calls.append(kwargs)
        page = self.connections_pages[kwargs.get("pageToken")]
        return _FakeRequest(page)

    def get(self, **kwargs: Any) -> _FakeRequest:
        self.get_calls.append(kwargs)
        return _FakeRequest(self.get_response)

    def createContact(self, **kwargs: Any) -> _FakeRequest:
        self.create_calls.append(kwargs)
        return _FakeRequest(self.create_response)

    def updateContact(self, **kwargs: Any) -> _FakeRequest:
        self.update_calls.append(kwargs)
        return _FakeRequest(self.update_response)

    def deleteContact(self, **kwargs: Any) -> _FakeRequest:
        self.delete_calls.append(kwargs)
        return _FakeRequest(self.delete_response)


class FakeSettingsResource:
    """Stands in for ``gmail_service.users().settings()``."""

    def __init__(self) -> None:
        self.vacation_response: dict = {}
        self.update_calls: list[dict] = []

    def getVacation(self, **kwargs: Any) -> _FakeRequest:
        return _FakeRequest(self.vacation_response)

    def updateVacation(self, **kwargs: Any) -> _FakeRequest:
        self.update_calls.append(kwargs)
        return _FakeRequest(kwargs.get("body"))


class FakeBackend(GoogleContactsMixin, GoogleSettingsMixin):
    """Minimal host object providing ``self._client`` and ``self.account``."""

    def __init__(self, people: FakePeopleResource, gmail_settings: FakeSettingsResource) -> None:
        self.account = AccountConfig(email="me@example.com", provider="google", alias="personal")
        self._client = MagicMock()
        self._client.people.return_value.people.return_value = people
        self._client.gmail.return_value.users.return_value.settings.return_value = gmail_settings


@pytest.fixture
def people() -> FakePeopleResource:
    return FakePeopleResource()


@pytest.fixture
def gmail_settings() -> FakeSettingsResource:
    return FakeSettingsResource()


@pytest.fixture
def backend(people: FakePeopleResource, gmail_settings: FakeSettingsResource) -> FakeBackend:
    return FakeBackend(people, gmail_settings)


# ---------------------------------------------------------------------------
# Contacts: shape parity
# ---------------------------------------------------------------------------


PERSON = {
    "resourceName": "people/c123",
    "etag": "%EtagValue123",
    "names": [{"displayName": "Ada Lovelace", "givenName": "Ada", "familyName": "Lovelace"}],
    "emailAddresses": [{"value": "ada@example.com"}],
    "phoneNumbers": [{"value": "+1-555-0100"}],
    "organizations": [{"name": "Analytical Engines Ltd", "title": "Engineer"}],
}

#: The exact key set h8.contacts._contact_to_dict produces (list/get/update shape).
EXPECTED_CONTACT_KEYS = {
    "id",
    "changekey",
    "display_name",
    "given_name",
    "surname",
    "email",
    "phone",
    "company",
    "job_title",
}


def test_list_contacts_shape_matches_ews_contacts(backend, people):
    people.connections_pages = {None: {"connections": [PERSON]}}

    result = backend.list_contacts(limit=10)

    assert len(result) == 1
    contact = result[0]
    assert set(contact.keys()) == EXPECTED_CONTACT_KEYS
    assert contact["id"] == "people/c123"
    assert contact["changekey"] == "%EtagValue123"
    assert contact["display_name"] == "Ada Lovelace"
    assert contact["given_name"] == "Ada"
    assert contact["surname"] == "Lovelace"
    assert contact["email"] == "ada@example.com"
    assert contact["phone"] == "+1-555-0100"
    assert contact["company"] == "Analytical Engines Ltd"
    assert contact["job_title"] == "Engineer"


def test_list_contacts_requests_expected_person_fields(backend, people):
    backend.list_contacts(limit=5)
    assert len(people.connections_list_calls) == 1
    call = people.connections_list_calls[0]
    assert call["resourceName"] == "people/me"
    assert call["personFields"] == "names,emailAddresses,phoneNumbers,organizations"


def test_list_contacts_zero_limit_returns_empty_without_calling_api(backend, people):
    assert backend.list_contacts(limit=0) == []
    assert people.connections_list_calls == []


def test_list_contacts_search_filters_client_side(backend, people):
    other = {
        "resourceName": "people/c456",
        "etag": "e2",
        "names": [{"displayName": "Bob Smith"}],
        "emailAddresses": [{"value": "bob@example.com"}],
    }
    people.connections_pages = {None: {"connections": [PERSON, other]}}

    result = backend.list_contacts(limit=10, search="ada")
    assert len(result) == 1
    assert result[0]["display_name"] == "Ada Lovelace"

    result = backend.list_contacts(limit=10, search="bob@example.com")
    assert len(result) == 1
    assert result[0]["display_name"] == "Bob Smith"


def test_list_contacts_stops_at_limit_across_pages(backend, people):
    page1 = {"connections": [PERSON], "nextPageToken": "tok2"}
    page2 = {"connections": [dict(PERSON, resourceName="people/c999")]}
    people.connections_pages = {None: page1, "tok2": page2}

    result = backend.list_contacts(limit=1)
    assert len(result) == 1
    # Only the first page should have been fetched since limit=1 was hit.
    assert len(people.connections_list_calls) == 1


def test_get_contact_shape(backend, people):
    people.get_response = PERSON
    contact = backend.get_contact("c123")
    assert set(contact.keys()) == EXPECTED_CONTACT_KEYS
    assert contact["id"] == "people/c123"
    # Bare id gets normalized to a full resourceName.
    assert people.get_calls[0]["resourceName"] == "people/c123"


def test_get_contact_accepts_full_resource_name(backend, people):
    people.get_response = PERSON
    backend.get_contact("people/c123")
    assert people.get_calls[0]["resourceName"] == "people/c123"


def test_get_contact_not_found(backend, people, monkeypatch):
    from googleapiclient.errors import HttpError

    def _raise(**kwargs):
        return _RaisingRequest(HttpError(resp=MagicMock(status=404), content=b"{}"))

    monkeypatch.setattr(people, "get", _raise)
    result = backend.get_contact("missing")
    assert result == {"error": "Contact not found"}


def test_create_contact_shape_mirrors_ews_create(backend, people):
    people.create_response = {"resourceName": "people/c789", "etag": "e3"}

    result = backend.create_contact(
        {"name": "Grace Hopper", "email": "grace@example.com", "phone": "555-1"}
    )

    # h8.contacts.create_contact returns exactly this narrower shape (not the
    # full list/get/update shape).
    assert set(result.keys()) == {"id", "changekey", "name", "email"}
    assert result["id"] == "people/c789"
    assert result["changekey"] == "e3"
    assert result["name"] == "Grace Hopper"
    assert result["email"] == "grace@example.com"

    body = people.create_calls[0]["body"]
    assert body["names"] == [{"displayName": "Grace Hopper", "givenName": "Grace", "familyName": "Hopper"}]
    assert body["emailAddresses"] == [{"value": "grace@example.com"}]
    assert body["phoneNumbers"] == [{"value": "555-1"}]


def test_delete_contact_success(backend, people):
    people.delete_response = {}
    result = backend.delete_contact("c123")
    assert result == {"success": True, "id": "c123"}
    assert people.delete_calls[0]["resourceName"] == "people/c123"


def test_delete_contact_not_found(backend, people, monkeypatch):
    from googleapiclient.errors import HttpError

    def _raise(**kwargs):
        return _RaisingRequest(HttpError(resp=MagicMock(status=404), content=b"{}"))

    monkeypatch.setattr(people, "deleteContact", _raise)
    result = backend.delete_contact("missing")
    assert result == {"success": False, "error": "Contact not found"}


# ---------------------------------------------------------------------------
# Contacts: update sends correct updatePersonFields + etag
# ---------------------------------------------------------------------------


def test_update_contact_uses_passed_changekey_without_refetch(backend, people):
    # `email` needs no merge with existing data, so with an explicit changekey
    # there is no reason to re-fetch the contact at all.
    people.update_response = dict(PERSON, emailAddresses=[{"value": "changed@example.com"}])

    backend.update_contact(
        "c123", {"email": "changed@example.com", "changekey": "etag-from-caller"}
    )

    assert people.get_calls == []  # no re-fetch needed
    call = people.update_calls[0]
    assert call["resourceName"] == "people/c123"
    assert call["updatePersonFields"] == "emailAddresses"
    assert call["body"]["etag"] == "etag-from-caller"
    assert call["body"]["emailAddresses"] == [{"value": "changed@example.com"}]


def test_update_contact_refetches_etag_when_absent(backend, people):
    people.get_response = PERSON
    people.update_response = PERSON

    backend.update_contact("c123", {"email": "new@example.com"})

    assert len(people.get_calls) == 1  # fetched once to get the etag
    call = people.update_calls[0]
    assert call["updatePersonFields"] == "emailAddresses"
    assert call["body"]["etag"] == "%EtagValue123"
    assert call["body"]["emailAddresses"] == [{"value": "new@example.com"}]


def test_update_contact_multiple_fields_builds_combined_mask(backend, people):
    people.get_response = PERSON
    people.update_response = PERSON

    backend.update_contact(
        "c123",
        {"email": "new@example.com", "company": "NewCo", "job_title": "Lead"},
    )

    call = people.update_calls[0]
    assert call["updatePersonFields"] == "emailAddresses,organizations"
    assert call["body"]["organizations"] == [{"name": "NewCo", "title": "Lead"}]


def test_update_contact_merges_partial_name_fields(backend, people):
    people.get_response = PERSON  # given_name=Ada, surname=Lovelace
    people.update_response = PERSON

    backend.update_contact("c123", {"display_name": "A. Lovelace"})

    call = people.update_calls[0]
    assert call["body"]["names"] == [
        {"displayName": "A. Lovelace", "givenName": "Ada", "familyName": "Lovelace"}
    ]


def test_update_contact_no_recognized_fields_returns_current(backend, people):
    people.get_response = PERSON
    result = backend.update_contact("c123", {"unrelated_key": "x"})
    assert result["id"] == "people/c123"
    assert people.update_calls == []


# ---------------------------------------------------------------------------
# OOF: Gmail vacation -> h8 shape
# ---------------------------------------------------------------------------


def test_get_oof_settings_disabled(backend, gmail_settings):
    gmail_settings.vacation_response = {"enableAutoReply": False}
    result = backend.get_oof_settings()
    assert result["state"] == "Disabled"
    assert result["enabled"] is False
    assert result["scheduled"] is False
    assert result["external_audience"] == "All"
    assert "start" not in result
    assert "internal_reply" not in result


def test_get_oof_settings_enabled_no_window(backend, gmail_settings):
    gmail_settings.vacation_response = {
        "enableAutoReply": True,
        "responseBodyHtml": "<p>Out until further notice</p>",
        "restrictToDomain": False,
    }
    result = backend.get_oof_settings()
    assert result["state"] == "Enabled"
    assert result["enabled"] is True
    assert result["scheduled"] is False
    assert result["external_audience"] == "All"
    assert result["internal_reply"] == "<p>Out until further notice</p>"
    assert result["external_reply"] == "<p>Out until further notice</p>"


def test_get_oof_settings_scheduled_with_window(backend, gmail_settings):
    gmail_settings.vacation_response = {
        "enableAutoReply": True,
        "responseBodyHtml": "Back on Monday",
        "restrictToDomain": True,
        "startTime": "1735689600000",  # 2025-01-01T00:00:00Z
        "endTime": "1735776000000",  # 2025-01-02T00:00:00Z
    }
    result = backend.get_oof_settings()
    assert result["state"] == "Scheduled"
    assert result["enabled"] is True
    assert result["scheduled"] is True
    assert result["external_audience"] == "None"
    assert result["start"].startswith("2025-01-01")
    assert result["end"].startswith("2025-01-02")


# ---------------------------------------------------------------------------
# OOF: h8 shape -> Gmail vacation (set/enable/schedule/disable)
# ---------------------------------------------------------------------------


def test_set_oof_settings_enabled_sends_enable_auto_reply(backend, gmail_settings):
    gmail_settings.vacation_response = {"enableAutoReply": True, "responseBodyHtml": "hi"}

    backend.set_oof_settings(
        state="Enabled", internal_reply="hi", external_reply="hi", external_audience="All"
    )

    body = gmail_settings.update_calls[0]["body"]
    assert body["enableAutoReply"] is True
    assert body["responseBodyHtml"] == "hi"
    assert body["restrictToDomain"] is False
    assert "startTime" not in body
    assert "endTime" not in body


def test_set_oof_settings_scheduled_sends_epoch_ms_window(backend, gmail_settings):
    gmail_settings.vacation_response = {"enableAutoReply": True}

    backend.set_oof_settings(
        state="Scheduled",
        start="2025-01-01T00:00:00+00:00",
        end="2025-01-02T00:00:00+00:00",
        internal_reply="away",
        external_audience="Known",
    )

    body = gmail_settings.update_calls[0]["body"]
    assert body["enableAutoReply"] is True
    assert body["startTime"] == "1735689600000"
    assert body["endTime"] == "1735776000000"
    # "Known" has no Gmail equivalent; best-effort maps to restrictToDomain=True.
    assert body["restrictToDomain"] is True


def test_disable_oof_sends_enable_auto_reply_false(backend, gmail_settings):
    gmail_settings.vacation_response = {"enableAutoReply": False}
    backend.disable_oof()
    body = gmail_settings.update_calls[0]["body"]
    assert body == {"enableAutoReply": False}


def test_enable_oof_defaults_external_reply_to_internal(backend, gmail_settings):
    gmail_settings.vacation_response = {"enableAutoReply": True}
    backend.enable_oof(internal_reply="internal message")
    body = gmail_settings.update_calls[0]["body"]
    assert body["responseBodyHtml"] == "internal message"


def test_schedule_oof_round_trip(backend, gmail_settings):
    # After scheduling, get_oof_settings (called internally to build the
    # return value) should reflect back a Scheduled state.
    gmail_settings.vacation_response = {
        "enableAutoReply": True,
        "responseBodyHtml": "on leave",
        "restrictToDomain": False,
        "startTime": "1735689600000",
        "endTime": "1735776000000",
    }

    result = backend.schedule_oof(
        start="2025-01-01T00:00:00+00:00",
        end="2025-01-02T00:00:00+00:00",
        internal_reply="on leave",
    )

    assert result["state"] == "Scheduled"
    assert result["scheduled"] is True


# ---------------------------------------------------------------------------
# Inbox rules: unsupported (defense-in-depth)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "call",
    [
        lambda b: b.list_rules(),
        lambda b: b.get_rule("r1"),
        lambda b: b.create_rule("name"),
        lambda b: b.update_rule("r1", display_name="x"),
        lambda b: b.enable_rule("r1"),
        lambda b: b.disable_rule("r1"),
        lambda b: b.delete_rule("r1"),
    ],
)
def test_rule_methods_raise_backend_not_supported(backend, call):
    with pytest.raises(BackendNotSupported) as exc_info:
        call(backend)
    assert "google" in str(exc_info.value)
    assert exc_info.value.capability == "rules"
