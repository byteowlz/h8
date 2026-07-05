"""Tests for the /auth/* endpoints with the OAuth layer mocked.

No MSAL/google-auth or network is exercised: the endpoints import ``h8.oauth``
lazily, so we patch attributes on that module (and ``h8.service.resolve_account``
/ ``_list_account_configs``) to drive each flow. The registry cache clear is
patched to a no-op so tests stay isolated.
"""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from h8 import service
from h8.accounts import AccountResolutionError
from h8.providers.base import AccountConfig


@pytest.fixture(autouse=True)
def _no_cache_clear():
    """Neutralize the registry cache clear the endpoints trigger on success."""
    with patch("h8.providers.registry.clear_cache"):
        yield


def _client() -> TestClient:
    # Constructed without the context manager so the lifespan (startup token
    # refresh) never runs.
    return TestClient(service.app)


def _ews_account() -> AccountConfig:
    return AccountConfig(email="work@example.com", provider="ews", alias="work")


def _google_account() -> AccountConfig:
    return AccountConfig(
        email="me@gmail.com", provider="google", alias="personal"
    )


def test_auth_accounts_lists_configured_accounts():
    accounts = [_ews_account(), _google_account()]
    statuses = {
        "work@example.com": {"logged_in": True, "expires_at": 1234.0, "provider": "ews"},
        "me@gmail.com": {"logged_in": False, "expires_at": None, "provider": "google"},
    }
    with patch("h8.service._list_account_configs", return_value=accounts), patch(
        "h8.oauth.login_status", side_effect=lambda a: statuses[a.email]
    ):
        resp = _client().get("/auth/accounts")

    assert resp.status_code == 200
    assert resp.json() == [
        {
            "alias": "work",
            "email": "work@example.com",
            "provider": "ews",
            "logged_in": True,
            "expires_at": 1234.0,
        },
        {
            "alias": "personal",
            "email": "me@gmail.com",
            "provider": "google",
            "logged_in": False,
            "expires_at": None,
        },
    ]


def test_auth_login_device_code_flow():
    device = SimpleNamespace(
        session_id="sess-ews",
        verification_url="https://microsoft.com/devicelogin",
        user_code="ABCD-EFGH",
        expires_at=0.0,
    )
    with patch("h8.service.resolve_account", return_value=_ews_account()), patch(
        "h8.oauth.start_device_login", return_value=device
    ):
        resp = _client().post("/auth/login", json={"account": "work"})

    assert resp.status_code == 200
    assert resp.json() == {
        "flow": "device_code",
        "session_id": "sess-ews",
        "verification_url": "https://microsoft.com/devicelogin",
        "user_code": "ABCD-EFGH",
    }


def test_auth_login_google_auth_url_flow():
    session = SimpleNamespace(session_id="sess-goog", auth_url="https://accounts.google.com/o/oauth2/auth?x=1")
    with patch("h8.service.resolve_account", return_value=_google_account()), patch(
        "h8.oauth.google.start_login", return_value=session
    ):
        resp = _client().post("/auth/login", json={"account": "personal"})

    assert resp.status_code == 200
    assert resp.json() == {
        "flow": "auth_url",
        "session_id": "sess-goog",
        "auth_url": "https://accounts.google.com/o/oauth2/auth?x=1",
    }


def test_auth_login_unknown_account_returns_400():
    with patch(
        "h8.service.resolve_account",
        side_effect=AccountResolutionError("Unknown account 'nope'"),
    ):
        resp = _client().post("/auth/login", json={"account": "nope"})

    assert resp.status_code == 400
    assert "Unknown account" in resp.json()["detail"]


def test_auth_poll_pending():
    with patch("h8.oauth.poll_device_login", return_value="pending"):
        resp = _client().get("/auth/login/sess-ews")

    assert resp.status_code == 200
    assert resp.json() == {"status": "pending"}


def test_auth_poll_done_clears_cache():
    with patch("h8.oauth.poll_device_login", return_value="done"), patch(
        "h8.providers.registry.clear_cache"
    ) as clear:
        resp = _client().get("/auth/login/sess-ews")

    assert resp.status_code == 200
    assert resp.json() == {"status": "done"}
    clear.assert_called_once()


def test_auth_poll_falls_through_to_google_and_normalizes_error():
    with patch(
        "h8.oauth.poll_device_login", return_value="error: unknown session"
    ), patch("h8.oauth.poll_login", return_value="error: boom"):
        resp = _client().get("/auth/login/sess-goog")

    assert resp.status_code == 200
    assert resp.json() == {"status": "error", "detail": "boom"}


def test_auth_finish_url_login():
    with patch("h8.oauth.finish_url_login", return_value="done") as finish:
        resp = _client().post(
            "/auth/login/sess-goog/finish",
            json={"redirect_url": "http://localhost/?code=xyz"},
        )

    assert resp.status_code == 200
    assert resp.json() == {"status": "done"}
    finish.assert_called_once_with("sess-goog", "http://localhost/?code=xyz")


def test_auth_logout():
    with patch("h8.service.resolve_account", return_value=_ews_account()), patch(
        "h8.oauth.logout"
    ) as logout:
        resp = _client().post("/auth/logout", json={"account": "work"})

    assert resp.status_code == 200
    assert resp.json() == {"status": "ok", "account": "work"}
    logout.assert_called_once()
