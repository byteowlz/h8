"""Tests for the auth module."""

import time
import threading
import pytest
from unittest.mock import patch, MagicMock

from h8.auth import (
    AccountManager,
    CachedAccount,
    get_token,
    get_account,
    refresh_account,
    clear_account_cache,
    get_account_manager,
    DEFAULT_TOKEN_LIFETIME_SECONDS,
    TOKEN_REFRESH_MARGIN_SECONDS,
)


class TestCachedAccount:
    """Tests for CachedAccount class."""

    def test_not_expired_when_new(self):
        """A newly created cached account should not be expired."""
        mock_account = MagicMock()
        cached = CachedAccount(
            account=mock_account,
            created_at=time.time(),
            email="test@example.com",
        )
        assert not cached.is_expired()

    def test_expired_when_old(self):
        """A cached account past its lifetime should be expired."""
        mock_account = MagicMock()
        # Created longer ago than the token lifetime
        old_time = time.time() - DEFAULT_TOKEN_LIFETIME_SECONDS - 100
        cached = CachedAccount(
            account=mock_account,
            created_at=old_time,
            email="test@example.com",
        )
        assert cached.is_expired()

    def test_expires_near_margin(self):
        """Account should be considered expired when within refresh margin."""
        mock_account = MagicMock()
        # Created just before the refresh margin
        margin_time = time.time() - (
            DEFAULT_TOKEN_LIFETIME_SECONDS - TOKEN_REFRESH_MARGIN_SECONDS + 10
        )
        cached = CachedAccount(
            account=mock_account,
            created_at=margin_time,
            email="test@example.com",
        )
        assert cached.is_expired()


class TestAccountManager:
    """Tests for AccountManager class."""

    @patch("h8.auth.get_token")
    @patch("h8.auth.Account")
    @patch("h8.auth.Configuration")
    @patch("h8.auth.OAuth2AuthorizationCodeCredentials")
    def test_get_account_creates_new(
        self, mock_creds, mock_config, mock_account, mock_get_token
    ):
        """get_account should create a new account when none is cached."""
        mock_get_token.return_value = "test_token"
        mock_account_instance = MagicMock()
        mock_account.return_value = mock_account_instance

        manager = AccountManager()
        result = manager.get_account("test@example.com")

        assert result == mock_account_instance
        mock_get_token.assert_called_once_with("test@example.com")

    @patch("h8.auth.get_token")
    @patch("h8.auth.Account")
    @patch("h8.auth.Configuration")
    @patch("h8.auth.OAuth2AuthorizationCodeCredentials")
    def test_get_account_returns_cached(
        self, mock_creds, mock_config, mock_account, mock_get_token
    ):
        """get_account should return cached account if not expired."""
        mock_get_token.return_value = "test_token"
        mock_account_instance = MagicMock()
        mock_account.return_value = mock_account_instance

        manager = AccountManager()

        # First call creates the account
        result1 = manager.get_account("test@example.com")
        # Second call should return cached
        result2 = manager.get_account("test@example.com")

        assert result1 == result2
        # Token should only be fetched once since account is cached
        assert mock_get_token.call_count == 1

    @patch("h8.auth.get_token")
    @patch("h8.auth.Account")
    @patch("h8.auth.Configuration")
    @patch("h8.auth.OAuth2AuthorizationCodeCredentials")
    def test_refresh_account_creates_new(
        self, mock_creds, mock_config, mock_account, mock_get_token
    ):
        """refresh_account should always create a fresh account."""
        mock_get_token.return_value = "test_token"
        mock_account_instance = MagicMock()
        mock_account.return_value = mock_account_instance

        manager = AccountManager()

        # First get
        manager.get_account("test@example.com")
        # Force refresh
        manager.refresh_account("test@example.com")

        # Token should be fetched twice
        assert mock_get_token.call_count == 2

    def test_clear_cache(self):
        """clear_cache should remove all cached accounts."""
        manager = AccountManager()
        # Manually add a cached account
        mock_account = MagicMock()
        manager._accounts["test@example.com"] = CachedAccount(
            account=mock_account,
            created_at=time.time(),
            email="test@example.com",
        )

        manager.clear_cache()
        assert len(manager._accounts) == 0

    def test_get_cache_info(self):
        """get_cache_info should return information about cached accounts."""
        manager = AccountManager()
        mock_account = MagicMock()
        manager._accounts["test@example.com"] = CachedAccount(
            account=mock_account,
            created_at=time.time(),
            email="test@example.com",
        )

        info = manager.get_cache_info()
        assert "test@example.com" in info
        assert "age_seconds" in info["test@example.com"]
        assert "is_expired" in info["test@example.com"]

    @patch("h8.auth.get_token")
    @patch("h8.auth.Account")
    @patch("h8.auth.Configuration")
    @patch("h8.auth.OAuth2AuthorizationCodeCredentials")
    def test_thread_safety(self, mock_creds, mock_config, mock_account, mock_get_token):
        """Account manager should be thread-safe."""
        mock_get_token.return_value = "test_token"
        mock_account_instance = MagicMock()
        mock_account.return_value = mock_account_instance

        manager = AccountManager()
        results = []
        errors = []

        def get_account():
            try:
                result = manager.get_account("test@example.com")
                results.append(result)
            except Exception as e:
                errors.append(e)

        threads = [threading.Thread(target=get_account) for _ in range(10)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert len(errors) == 0
        assert len(results) == 10
        # All results should be the same cached account
        assert all(r == results[0] for r in results)


class TestGetToken:
    """Tests for get_token function."""

    @patch("subprocess.check_output")
    def test_get_token_success(self, mock_subprocess):
        """get_token should return trimmed token from oama."""
        mock_subprocess.return_value = b"  test_token_value  \n"

        token = get_token("test@example.com")

        assert token == "test_token_value"
        mock_subprocess.assert_called_once()

    @patch("subprocess.check_output")
    def test_get_token_subprocess_error(self, mock_subprocess):
        """get_token should raise on subprocess error."""
        import subprocess

        mock_subprocess.side_effect = subprocess.CalledProcessError(
            1, "oama", stderr=b"Authentication failed"
        )

        with pytest.raises(subprocess.CalledProcessError):
            get_token("test@example.com")


class TestModuleFunctions:
    """Tests for module-level convenience functions."""

    @patch("h8.auth.get_account_manager")
    def test_get_account_delegates_to_manager(self, mock_get_manager):
        """get_account should delegate to AccountManager."""
        mock_manager = MagicMock()
        mock_get_manager.return_value = mock_manager
        mock_account = MagicMock()
        mock_manager.get_account.return_value = mock_account

        result = get_account("test@example.com")

        assert result == mock_account
        mock_manager.get_account.assert_called_once_with("test@example.com")

    @patch("h8.auth.get_account_manager")
    def test_refresh_account_delegates_to_manager(self, mock_get_manager):
        """refresh_account should delegate to AccountManager."""
        mock_manager = MagicMock()
        mock_get_manager.return_value = mock_manager
        mock_account = MagicMock()
        mock_manager.refresh_account.return_value = mock_account

        result = refresh_account("test@example.com")

        assert result == mock_account
        mock_manager.refresh_account.assert_called_once_with("test@example.com")

    @patch("h8.auth.get_account_manager")
    def test_clear_account_cache_delegates_to_manager(self, mock_get_manager):
        """clear_account_cache should delegate to AccountManager."""
        mock_manager = MagicMock()
        mock_get_manager.return_value = mock_manager

        clear_account_cache()

        mock_manager.clear_cache.assert_called_once()
