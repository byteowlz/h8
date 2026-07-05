"""Contract tests for the provider seam.

Exercises the FastAPI routes against a :class:`FakeBackend` so that no network
or exchangelib code is touched. The registry lookup (``h8.service.get_backend``)
is patched to return the fake, which lets us assert:

- mail/calendar/contacts routes delegate to the backend and preserve shapes
- ``GET /capabilities`` reports provider + capabilities
- unsupported capabilities yield HTTP 501 with ``missing_capability``
- ``BackendAuthError`` triggers exactly one ``refresh()`` then succeeds
- ``BackendBusyError`` triggers backoff and eventual success
"""

from __future__ import annotations

from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from h8 import service
from h8.providers.base import (
    ALL_CAPABILITIES,
    CAP_MAIL,
    CAP_OOF,
    AccountConfig,
    Backend,
    BackendAuthError,
    BackendBusyError,
)


class FakeBackend(Backend):
    """In-memory backend used to drive the routes without any provider SDK."""

    def __init__(
        self,
        capabilities=ALL_CAPABILITIES,
        fail_auth_times: int = 0,
        busy_times: int = 0,
    ) -> None:
        self.account = AccountConfig(
            email="fake@example.com", provider="ews", alias="fake"
        )
        self.provider = "ews"
        self.capabilities = capabilities
        self.refresh_count = 0
        self._auth_fails_remaining = fail_auth_times
        self._busy_remaining = busy_times

    def refresh(self) -> None:
        self.refresh_count += 1
        # A refresh "fixes" a stale-auth condition.
        self._auth_fails_remaining = 0

    def _maybe_fail(self) -> None:
        if self._auth_fails_remaining > 0:
            self._auth_fails_remaining -= 1
            raise BackendAuthError("stale token")
        if self._busy_remaining > 0:
            self._busy_remaining -= 1
            raise BackendBusyError("throttled", retry_after=0)

    # -- Mail --
    def list_messages(self, folder="inbox", limit=20, unread=False):
        return [{"id": "m1", "changekey": "ck1", "subject": "hello", "folder": folder}]

    def get_message(self, item_id, folder="inbox"):
        self._maybe_fail()
        return {"id": item_id, "changekey": "ck1", "subject": "hi"}

    def send_message(self, message_data):
        return {"sent": True, "to": message_data.get("to")}

    # -- Calendar --
    def list_events(self, days=7, from_date=None, to_date=None):
        return [{"id": "e1", "changekey": None, "subject": "standup"}]

    def create_event(self, event_data):
        return {"id": "e2", "changekey": None, "subject": event_data.get("subject")}

    # -- Contacts --
    def list_contacts(self, limit=100, search=None):
        return [{"id": "c1", "changekey": "ck2", "display_name": "Ada"}]

    # -- Settings --
    def get_oof_settings(self):
        return {"state": "Disabled"}


@pytest.fixture(autouse=True)
def _clear_cache():
    """Drop the service response cache between tests."""
    service.cache.clear()
    yield
    service.cache.clear()


@pytest.fixture(autouse=True)
def _no_auth(monkeypatch):
    """Use the auth escape hatch so contract tests focus on the provider seam."""
    monkeypatch.setenv("H8_SERVICE_NO_AUTH", "1")


def _client_with(backend: FakeBackend) -> TestClient:
    """A TestClient whose registry lookups return ``backend``.

    Instantiated WITHOUT the context-manager form so the lifespan (and its
    startup token refresh) never runs. ``base_url`` uses ``localhost`` so the
    Host-header allowlist passes.
    """
    return TestClient(service.app, base_url="http://localhost")


def test_mail_list_route():
    backend = FakeBackend()
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.get("/mail", params={"account": "fake"})
    assert resp.status_code == 200
    assert resp.json() == [
        {"id": "m1", "changekey": "ck1", "subject": "hello", "folder": "inbox"}
    ]


def test_mail_send_route():
    backend = FakeBackend()
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.post(
            "/mail/send",
            json={"to": ["a@example.com"], "subject": "hi", "body": "yo"},
        )
    assert resp.status_code == 200
    assert resp.json() == {"sent": True, "to": ["a@example.com"]}


def test_calendar_list_route():
    backend = FakeBackend()
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.get("/calendar")
    assert resp.status_code == 200
    assert resp.json() == [{"id": "e1", "changekey": None, "subject": "standup"}]


def test_calendar_create_route():
    backend = FakeBackend()
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.post(
            "/calendar",
            json={"subject": "Review", "start": "2026-07-06T10:00:00", "end": "2026-07-06T11:00:00"},
        )
    assert resp.status_code == 200
    assert resp.json() == {"id": "e2", "changekey": None, "subject": "Review"}


def test_contacts_list_route():
    backend = FakeBackend()
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.get("/contacts")
    assert resp.status_code == 200
    assert resp.json() == [{"id": "c1", "changekey": "ck2", "display_name": "Ada"}]


def test_capabilities_endpoint():
    backend = FakeBackend(capabilities=frozenset({CAP_MAIL, CAP_OOF}))
    with patch("h8.service.get_backend", return_value=backend), patch(
        "h8.service.resolve_account", return_value=backend.account
    ):
        client = _client_with(backend)
        resp = client.get("/capabilities", params={"account": "fake"})
    assert resp.status_code == 200
    body = resp.json()
    assert body["account"] == "fake"
    assert body["provider"] == "ews"
    assert sorted(body["capabilities"]) == sorted([CAP_MAIL, CAP_OOF])


def test_missing_capability_returns_501():
    backend = FakeBackend(capabilities=frozenset({CAP_MAIL}))  # no OOF
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.get("/oof")
    assert resp.status_code == 501
    body = resp.json()
    assert body["missing_capability"] == CAP_OOF
    assert "not supported" in body["detail"]


def test_auth_error_refreshes_once_then_succeeds():
    backend = FakeBackend(fail_auth_times=1)
    with patch("h8.service.get_backend", return_value=backend):
        client = _client_with(backend)
        resp = client.get("/mail/m1")
    assert resp.status_code == 200
    assert resp.json() == {"id": "m1", "changekey": "ck1", "subject": "hi"}
    assert backend.refresh_count == 1


def test_busy_error_backs_off_then_succeeds():
    backend = FakeBackend(busy_times=1)
    with patch("h8.service.get_backend", return_value=backend), patch(
        "h8.service.asyncio.sleep"
    ) as mock_sleep:
        client = _client_with(backend)
        resp = client.get("/mail/m1")
    assert resp.status_code == 200
    assert resp.json() == {"id": "m1", "changekey": "ck1", "subject": "hi"}
    # Backoff slept at least once (retry_after=0 from the busy error).
    assert mock_sleep.await_count >= 1
