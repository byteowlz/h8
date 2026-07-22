"""Tests for the Google headless (URL-paste) OAuth login.

Covers ``h8.oauth.google.finish_url_login``: the token exchange must pass a
bare authorization ``code`` to ``Flow.fetch_token`` -- NOT the pasted
``authorization_response`` URL. oauthlib >= 3.3 removed the loopback
(``127.0.0.1``/``localhost``) exemption from ``is_secure_transport``, so passing
the ``http://localhost`` redirect as ``authorization_response`` raises
``InsecureTransportError``. Exchanging a bare code skips that check. No network
runs: ``Flow`` is replaced by an in-memory fake.
"""

from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import patch

import pytest

from h8.oauth import LoginRequired
from h8.oauth import google as gauth


class _FakeFlow:
    """Records ``fetch_token`` kwargs; stands in for google-auth ``Flow``."""

    def __init__(self) -> None:
        self.calls: list[dict] = []
        self.credentials = SimpleNamespace(to_json=lambda: '{"token": "x"}')

    def fetch_token(self, **kwargs):  # noqa: ANN201
        self.calls.append(kwargs)


class _FakeStore:
    """In-memory stand-in for the OAuth TokenStore."""

    def __init__(self) -> None:
        self.data: dict[str, str] = {}

    def set(self, key: str, val: str) -> None:
        self.data[key] = val


def _seed(monkeypatch, flow: _FakeFlow, session_id: str = "sess-1",
          email: str = "user@gmail.com") -> str:
    """Inject a pending headless session backed by ``flow``."""
    with patch.object(gauth, "_get_store", lambda: _FakeStore()):
        monkeypatch.setattr(gauth, "_get_store", lambda: _FakeStore())
    with gauth._sessions_lock:
        gauth._sessions[session_id] = {
            "status": "pending",
            "detail": None,
            "email": email,
            "flow": flow,
        }
    return session_id


class TestFinishUrlLogin:
    """``finish_url_login`` exchanges a bare code, never the loopback URL."""

    def test_extracts_code_and_exchanges_it_directly(self, monkeypatch):
        """fetch_token receives ``code=``, never ``authorization_response=``."""
        flow = _FakeFlow()
        sid = _seed(monkeypatch, flow)
        url = (
            "http://localhost/?state=abc&iss=https://accounts.google.com"
            "&code=4/SYNTHETIC&scope=https://www.googleapis.com/auth/gmail.modify"
        )

        assert gauth.finish_url_login(sid, url) == "done"

        assert flow.calls == [{"code": "4/SYNTHETIC"}]
        assert all("authorization_response" not in c for c in flow.calls)

    def test_missing_code_raises_login_required(self, monkeypatch):
        """A pasted URL without ``code`` is reported, not exchanged."""
        flow = _FakeFlow()
        sid = _seed(monkeypatch, flow)

        with pytest.raises(LoginRequired):
            gauth.finish_url_login(sid, "http://localhost/?state=abc&scope=x")

        assert flow.calls == []  # exchange never attempted

    def test_unknown_session_raises_login_required(self, monkeypatch):
        """A session id that was never started is a hard error."""
        monkeypatch.setattr(gauth, "_get_store", lambda: _FakeStore())
        with pytest.raises(LoginRequired):
            gauth.finish_url_login("never-started", "http://localhost/?code=x")
