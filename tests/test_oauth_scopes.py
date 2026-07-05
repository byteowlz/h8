"""Pure-logic tests for the OAuth provider modules.

No network and no real MSAL/Google backends: the MSAL app and the token store
are patched. These cover scope selection, config resolution, and session
polling.
"""

from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from h8.oauth import LoginRequired
from h8.oauth import google, microsoft


def make_account(**overrides):
    """Build a minimal AccountLike stand-in."""
    base = dict(
        alias="work",
        email="user@example.com",
        provider="ews",
        client_id="test-client-id",
        tenant="organizations",
        extra={},
    )
    base.update(overrides)
    return SimpleNamespace(**base)


class TestMicrosoftScopes:
    def test_ews_scopes(self):
        assert microsoft._scopes_for("ews") == [
            "https://outlook.office365.com/EWS.AccessAsUser.All"
        ]

    def test_graph_scopes(self):
        scopes = microsoft._scopes_for("graph")
        for expected in (
            "Mail.ReadWrite",
            "Mail.Send",
            "Calendars.ReadWrite",
            "Contacts.ReadWrite",
            "MailboxSettings.ReadWrite",
            "People.Read",
            "User.ReadBasic.All",
        ):
            assert expected in scopes

    def test_graph_scopes_omit_offline_access(self):
        # MSAL adds offline_access itself.
        assert "offline_access" not in microsoft.GRAPH_SCOPES

    def test_invalid_resource_raises(self):
        with pytest.raises(ValueError):
            microsoft._scopes_for("pop3")

    def test_scopes_for_returns_fresh_list(self):
        first = microsoft._scopes_for("graph")
        first.append("mutated")
        assert "mutated" not in microsoft.GRAPH_SCOPES


class TestMicrosoftClientResolution:
    def test_client_id_from_account(self):
        assert microsoft._resolve_client_id(make_account(client_id="abc")) == "abc"

    def test_missing_client_id_raises(self):
        # DEFAULT_CLIENT_ID is empty, so an empty account client_id must raise.
        assert microsoft.DEFAULT_CLIENT_ID == ""
        with pytest.raises(LoginRequired):
            microsoft._resolve_client_id(make_account(client_id=""))

    def test_default_tenant(self):
        assert microsoft._resolve_tenant(make_account(tenant=None)) == "organizations"

    def test_tenant_from_account(self):
        assert microsoft._resolve_tenant(make_account(tenant="contoso")) == "contoso"


class TestGetMsToken:
    @patch("h8.oauth.microsoft._persist_cache")
    @patch("h8.oauth.microsoft._get_store")
    @patch("h8.oauth.microsoft._get_app")
    def test_no_cached_account_raises(self, mock_get_app, mock_store, mock_persist):
        app = MagicMock()
        app.get_accounts.return_value = []
        mock_get_app.return_value = (app, MagicMock())
        with pytest.raises(LoginRequired):
            microsoft.get_ms_token(make_account(), "ews")

    @patch("h8.oauth.microsoft._persist_cache")
    @patch("h8.oauth.microsoft._get_store")
    @patch("h8.oauth.microsoft._get_app")
    def test_silent_success(self, mock_get_app, mock_store, mock_persist):
        app = MagicMock()
        app.get_accounts.return_value = [{"username": "user@example.com"}]
        app.acquire_token_silent.return_value = {
            "access_token": "tok-123",
            "expires_in": 3600,
        }
        mock_get_app.return_value = (app, MagicMock())

        token = microsoft.get_ms_token(make_account(), "graph")

        assert token.token == "tok-123"
        assert token.expires_at > 0
        app.acquire_token_silent.assert_called_once()

    @patch("h8.oauth.microsoft._persist_cache")
    @patch("h8.oauth.microsoft._get_store")
    @patch("h8.oauth.microsoft._get_app")
    def test_silent_failure_raises(self, mock_get_app, mock_store, mock_persist):
        app = MagicMock()
        app.get_accounts.return_value = [{"username": "user@example.com"}]
        app.acquire_token_silent.return_value = None
        mock_get_app.return_value = (app, MagicMock())
        with pytest.raises(LoginRequired):
            microsoft.get_ms_token(make_account(), "ews")


class TestMicrosoftSessions:
    def test_poll_unknown_session(self):
        assert microsoft.poll_device_login("does-not-exist") == "error: unknown session"


class TestGoogleConfig:
    def test_scopes(self):
        assert google.SCOPES == [
            "https://www.googleapis.com/auth/gmail.modify",
            "https://www.googleapis.com/auth/gmail.settings.basic",
            "https://www.googleapis.com/auth/calendar",
            "https://www.googleapis.com/auth/contacts",
        ]

    def test_client_config_ok(self):
        account = make_account(
            provider="google",
            client_id="cid.apps.googleusercontent.com",
            extra={"client_secret": "secret"},
        )
        config = google._client_config(account)
        assert config["installed"]["client_id"] == "cid.apps.googleusercontent.com"
        assert config["installed"]["client_secret"] == "secret"

    def test_client_config_missing_secret_raises(self):
        account = make_account(provider="google", client_id="cid", extra={})
        with pytest.raises(LoginRequired):
            google._client_config(account)

    def test_client_config_missing_client_id_raises(self):
        account = make_account(
            provider="google", client_id="", extra={"client_secret": "s"}
        )
        with pytest.raises(LoginRequired):
            google._client_config(account)


class TestGoogleCredentials:
    @patch("h8.oauth.google._get_store")
    def test_missing_credentials_raises(self, mock_store):
        store = MagicMock()
        store.get.return_value = None
        mock_store.return_value = store
        with pytest.raises(LoginRequired):
            google.get_google_credentials(make_account(provider="google"))

    @patch("h8.oauth.google._get_store")
    def test_corrupt_credentials_raises(self, mock_store):
        store = MagicMock()
        store.get.return_value = "not-json{{"
        mock_store.return_value = store
        with pytest.raises(LoginRequired):
            google.get_google_credentials(make_account(provider="google"))


class TestGoogleSessions:
    def test_poll_unknown_session(self):
        assert google.poll_login("does-not-exist") == "error: unknown session"

    def test_finish_unknown_session_raises(self):
        with pytest.raises(LoginRequired):
            google.finish_url_login("nope", "http://localhost/?code=x")
