"""Integration tests for service authentication, key endpoints, and hardening.

These exercise the REAL 401/403 enforcement paths (auth is NOT disabled here),
the ``/keys`` CRUD endpoints, the Host-header allowlist, and the unsubscribe
SSRF guard (IP classifier + redirect re-check with a mocked httpx client).
"""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest
from fastapi.testclient import TestClient

from h8 import service, unsubscribe
from h8.providers.base import ALL_CAPABILITIES, AccountConfig
from h8.security import KeyStore
from h8.unsubscribe import (
    BlockedURLError,
    UnsubscribeLink,
    _fetch_with_ssrf_guard,
    _is_blocked_ip,
    _resolve_and_check_host,
)


class _FakeBackend:
    """Minimal backend for routes that pass the scope check."""

    def __init__(self) -> None:
        self.account = AccountConfig(email="w@example.com", provider="ews", alias="work")
        self.provider = "ews"
        self.capabilities = ALL_CAPABILITIES

    def refresh(self) -> None:  # pragma: no cover - not exercised
        pass

    def list_messages(self, folder="inbox", limit=20, unread=False):
        return [{"id": "m1", "changekey": "ck1", "subject": "hi", "folder": folder}]


@pytest.fixture
def keystore(tmp_path, monkeypatch):
    """A fresh, isolated KeyStore wired into the service module."""
    monkeypatch.delenv("H8_SERVICE_NO_AUTH", raising=False)
    # Direct audit writes into the temp dir instead of the real state home.
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
    store = KeyStore(path=tmp_path / "keys.json")
    monkeypatch.setattr(service, "key_store", store)
    service.cache.clear()
    return store


@pytest.fixture
def client(keystore):
    """A TestClient whose Host header ('localhost') passes the allowlist."""
    return TestClient(service.app, base_url="http://localhost")


def _auth(token: str) -> dict:
    return {"Authorization": f"Bearer {token}"}


# --- Authentication -------------------------------------------------------


def test_missing_header_is_401(client):
    resp = client.get("/mail")
    assert resp.status_code == 401
    assert "Authorization" in resp.json()["detail"]


def test_invalid_token_is_401(client):
    resp = client.get("/mail", headers=_auth("h8k_bogus"))
    assert resp.status_code == 401
    assert "Invalid" in resp.json()["detail"]


def test_insufficient_scope_is_403_with_required_scope(client, keystore):
    _rec, token = keystore.create_key("reader", ["mail:read"])
    # mail:read cannot POST /mail/send (needs mail:send).
    resp = client.post(
        "/mail/send",
        headers=_auth(token),
        json={"to": ["a@example.com"], "subject": "x", "body": "y"},
    )
    assert resp.status_code == 403
    body = resp.json()
    assert body["required_scope"] == "mail:send"
    assert "reader" in body["detail"]


def test_sufficient_scope_allows_request(client, keystore):
    _rec, token = keystore.create_key("reader", ["mail:read"])
    with patch("h8.service.get_backend", return_value=_FakeBackend()):
        resp = client.get("/mail", headers=_auth(token))
    assert resp.status_code == 200
    assert resp.json()[0]["id"] == "m1"


def test_wildcard_key_allows_everything(client, keystore):
    _rec, token = keystore.create_key("root", ["*:*"])
    with patch("h8.service.get_backend", return_value=_FakeBackend()):
        resp = client.get("/mail", headers=_auth(token))
    assert resp.status_code == 200


def test_account_restriction_blocks_foreign_account(client, keystore):
    _rec, token = keystore.create_key("work-only", ["mail:read"], accounts=["work"])
    resp = client.get("/mail", params={"account": "personal"}, headers=_auth(token))
    assert resp.status_code == 403
    assert "restricted" in resp.json()["detail"]


def test_account_restriction_allows_permitted_account(client, keystore):
    _rec, token = keystore.create_key("work-only", ["mail:read"], accounts=["work"])
    with patch("h8.service.get_backend", return_value=_FakeBackend()):
        resp = client.get("/mail", params={"account": "work"}, headers=_auth(token))
    assert resp.status_code == 200


def test_health_and_capabilities_are_unauthenticated(client):
    with patch("h8.service.get_cache_info", return_value={}):
        assert client.get("/health").status_code == 200


# --- Key management endpoints --------------------------------------------


def test_keys_crud_flow(client, keystore):
    _rec, admin_token = keystore.create_key("root", ["*:*"])
    # Create a key.
    resp = client.post(
        "/keys",
        headers=_auth(admin_token),
        json={"name": "agent", "scopes": ["mail:read"], "accounts": None},
    )
    assert resp.status_code == 200
    created = resp.json()
    assert created["token"].startswith("h8k_")
    assert created["name"] == "agent"
    new_id = created["id"]

    # List keys (hash never exposed).
    resp = client.get("/keys", headers=_auth(admin_token))
    assert resp.status_code == 200
    listed = resp.json()
    assert any(k["id"] == new_id for k in listed)
    assert all("hash" not in k for k in listed)

    # Revoke it.
    resp = client.delete(f"/keys/{new_id}", headers=_auth(admin_token))
    assert resp.status_code == 200
    assert resp.json() == {"status": "revoked", "id": new_id}


def test_keys_create_rejects_bad_scope(client, keystore):
    _rec, admin_token = keystore.create_key("root", ["*:*"])
    resp = client.post(
        "/keys",
        headers=_auth(admin_token),
        json={"name": "bad", "scopes": ["bogus:read"]},
    )
    assert resp.status_code == 422
    assert "bogus" in resp.json()["detail"]


def test_keys_requires_keys_scope(client, keystore):
    _rec, token = keystore.create_key("reader", ["mail:read"])
    resp = client.get("/keys", headers=_auth(token))
    assert resp.status_code == 403
    assert resp.json()["required_scope"] == "keys:read"


# --- Host-header allowlist ------------------------------------------------


def test_bad_host_header_is_400(keystore):
    _rec, token = keystore.create_key("root", ["*:*"])
    evil = TestClient(service.app, base_url="http://evil.example.com")
    resp = evil.get("/mail", headers=_auth(token))
    assert resp.status_code == 400
    assert "not allowed" in resp.json()["detail"]


# --- SSRF guard: IP classifier -------------------------------------------


@pytest.mark.parametrize(
    "ip,blocked",
    [
        ("127.0.0.1", True),
        ("10.0.0.5", True),
        ("172.16.4.4", True),
        ("192.168.1.1", True),
        ("169.254.169.254", True),  # cloud metadata endpoint
        ("0.0.0.0", True),
        ("::1", True),
        ("fe80::1", True),
        ("fc00::1", True),
        ("8.8.8.8", False),
        ("1.1.1.1", False),
        ("93.184.216.34", False),  # example.com
    ],
)
def test_is_blocked_ip(ip, blocked):
    assert _is_blocked_ip(ip) is blocked


def test_is_blocked_ip_unparseable_fails_closed():
    assert _is_blocked_ip("not-an-ip") is True


def test_resolve_and_check_host_blocks_private(monkeypatch):
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: [(2, 1, 6, "", ("10.1.2.3", 0))],
    )
    blocked, reason = _resolve_and_check_host("internal.example.com")
    assert blocked is True
    assert "10.1.2.3" in reason


def test_resolve_and_check_host_allows_public(monkeypatch):
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: [(2, 1, 6, "", ("93.184.216.34", 0))],
    )
    blocked, _reason = _resolve_and_check_host("example.com")
    assert blocked is False


# --- SSRF guard: redirect re-check ---------------------------------------


def test_fetch_follows_redirect_and_rechecks(monkeypatch):
    # First host public, redirect target host also public -> followed.
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: [(2, 1, 6, "", ("93.184.216.34", 0))],
    )
    client = MagicMock()
    redirect = MagicMock(status_code=302, headers={"location": "https://ok.example.com/done"})
    final = MagicMock(status_code=200, headers={})
    client.get.side_effect = [redirect, final]
    resp = _fetch_with_ssrf_guard(client, "https://start.example.com/u")
    assert resp is final
    assert client.get.call_count == 2


def test_fetch_blocks_redirect_to_private(monkeypatch):
    # start.example.com is public; the redirect points at a private host.
    resolutions = {
        "start.example.com": [(2, 1, 6, "", ("93.184.216.34", 0))],
        "internal": [(2, 1, 6, "", ("169.254.169.254", 0))],
    }
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: resolutions[host],
    )
    client = MagicMock()
    client.get.return_value = MagicMock(
        status_code=302, headers={"location": "http://internal/creds"}
    )
    with pytest.raises(BlockedURLError):
        _fetch_with_ssrf_guard(client, "https://start.example.com/u")


def test_fetch_blocks_initial_private_host(monkeypatch):
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: [(2, 1, 6, "", ("127.0.0.1", 0))],
    )
    client = MagicMock()
    with pytest.raises(BlockedURLError):
        _fetch_with_ssrf_guard(client, "http://localhost/u")
    client.get.assert_not_called()


def test_fetch_enforces_max_hops(monkeypatch):
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: [(2, 1, 6, "", ("93.184.216.34", 0))],
    )
    client = MagicMock()
    # Always redirect -> should give up after MAX_REDIRECT_HOPS.
    client.get.return_value = MagicMock(
        status_code=302, headers={"location": "https://loop.example.com/next"}
    )
    with pytest.raises(BlockedURLError, match="redirect hops"):
        _fetch_with_ssrf_guard(client, "https://loop.example.com/start")


def test_visit_blocked_link_returns_skipped(monkeypatch):
    monkeypatch.setattr(
        unsubscribe.socket,
        "getaddrinfo",
        lambda host, port: [(2, 1, 6, "", ("127.0.0.1", 0))],
    )
    links = [UnsubscribeLink(url="http://localhost/unsub", source="header")]
    result = unsubscribe._visit_unsubscribe_link(
        "msg1", "sender@example.com", "Subject", links, trusted_domains=[]
    )
    assert result.status == "skipped"
    assert "blocked" in (result.error or "")
