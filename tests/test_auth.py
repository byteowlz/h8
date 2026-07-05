"""Tests for the auth module.

Account construction and caching live in the provider registry
(``h8.providers.registry``) and the EWS backend (``h8.providers.ews``). OAuth
token acquisition lives in ``h8.oauth`` (MSAL / google-auth) -- the external
``oama`` binary and its GPG machinery have been removed. What remains in
``h8.auth`` is the :class:`AuthLoginRequired` exception plus thin
``get_account`` / ``refresh_account`` shims that delegate to the registry for the
legacy direct CLI.
"""

from unittest.mock import MagicMock, patch

import pytest

from h8.auth import (
    AuthLoginRequired,
    clear_account_cache,
    get_account,
    refresh_account,
)
from h8.providers.base import BackendAuthError


class TestAuthLoginRequired:
    """AuthLoginRequired belongs to the BackendAuthError family."""

    def test_is_backend_auth_error_subclass(self):
        """It must subclass BackendAuthError so routes can catch either."""
        assert issubclass(AuthLoginRequired, BackendAuthError)

    def test_carries_message(self):
        err = AuthLoginRequired("run h8 auth login work")
        assert "run h8 auth login work" in str(err)


class TestAccountShims:
    """The compatibility shims delegate to the provider registry."""

    @patch("h8.providers.registry.get_backend")
    def test_get_account_returns_ews_account(self, mock_get_backend):
        """get_account should return the EWS backend's exchangelib account."""
        backend = MagicMock()
        backend.provider = "ews"
        backend.ews_account = MagicMock()
        mock_get_backend.return_value = backend

        result = get_account("test@example.com")

        assert result is backend.ews_account
        mock_get_backend.assert_called_once_with("test@example.com")

    @patch("h8.providers.registry.get_backend")
    def test_get_account_rejects_non_ews(self, mock_get_backend):
        """get_account should refuse a non-EWS backend (direct CLI is EWS-only)."""
        backend = MagicMock()
        backend.provider = "google"
        mock_get_backend.return_value = backend

        with pytest.raises(RuntimeError):
            get_account("someone@gmail.com")

    @patch("h8.providers.registry.get_backend")
    def test_refresh_account_refreshes_and_returns(self, mock_get_backend):
        """refresh_account should refresh the backend and return its account."""
        backend = MagicMock()
        backend.provider = "ews"
        backend.ews_account = MagicMock()
        mock_get_backend.return_value = backend

        result = refresh_account("test@example.com")

        backend.refresh.assert_called_once()
        assert result is backend.ews_account

    @patch("h8.providers.registry.clear_cache")
    def test_clear_account_cache_delegates(self, mock_clear):
        """clear_account_cache should clear the registry cache."""
        clear_account_cache()
        mock_clear.assert_called_once()
