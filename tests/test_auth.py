"""Tests for the auth module.

Account construction and caching moved into the provider registry
(``h8.providers.registry``) and the EWS backend (``h8.providers.ews``). What
remains in ``h8.auth`` is the oama token machinery plus thin ``get_account`` /
``refresh_account`` shims that delegate to the registry for the legacy direct CLI.
"""

import subprocess
from unittest.mock import MagicMock, patch

import pytest

from h8.auth import (
    clear_account_cache,
    get_account,
    get_token,
    refresh_account,
)


class TestGetToken:
    """Tests for get_token function (oama access)."""

    @patch("h8.auth.ensure_oama")
    @patch("subprocess.check_output")
    def test_get_token_success(self, mock_subprocess, mock_ensure):
        """get_token should return the trimmed token from oama."""
        mock_subprocess.return_value = b"  test_token_value  \n"

        token = get_token("test@example.com")

        assert token == "test_token_value"
        mock_subprocess.assert_called_once()

    @patch("h8.auth.ensure_oama")
    @patch("subprocess.check_output")
    def test_get_token_subprocess_error(self, mock_subprocess, mock_ensure):
        """get_token should raise on subprocess error after a failed renew."""
        mock_subprocess.side_effect = subprocess.CalledProcessError(
            1, "oama", stderr=b"Authentication failed"
        )

        with pytest.raises(subprocess.CalledProcessError):
            get_token("test@example.com", attempt_renew=False)


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
