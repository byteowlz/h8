"""Authentication and EWS account connection with automatic token refresh."""

import logging
import subprocess
import threading
import time
from dataclasses import dataclass, field
from typing import Optional

from exchangelib import Account, Configuration, DELEGATE
from exchangelib import OAuth2AuthorizationCodeCredentials

log = logging.getLogger(__name__)

# Token refresh interval - refresh 5 minutes before expected expiry
# OAuth tokens typically expire after 1 hour, we refresh at 55 minutes
TOKEN_REFRESH_MARGIN_SECONDS = 5 * 60
DEFAULT_TOKEN_LIFETIME_SECONDS = 55 * 60


@dataclass
class CachedAccount:
    """Cached EWS account with token expiry tracking."""

    account: Account
    created_at: float
    email: str
    token_lifetime: float = DEFAULT_TOKEN_LIFETIME_SECONDS

    def is_expired(self) -> bool:
        """Check if the token is likely expired or about to expire."""
        age = time.time() - self.created_at
        return age >= (self.token_lifetime - TOKEN_REFRESH_MARGIN_SECONDS)


class AccountManager:
    """Manages EWS account connections with automatic token refresh."""

    def __init__(self):
        self._accounts: dict[str, CachedAccount] = {}
        self._lock = threading.Lock()

    def get_account(self, email: str) -> Account:
        """Get an EWS account, refreshing the token if needed."""
        with self._lock:
            cached = self._accounts.get(email)

            if cached is not None and not cached.is_expired():
                log.debug(
                    "Using cached account for %s (age: %.1fs)",
                    email,
                    time.time() - cached.created_at,
                )
                return cached.account

            if cached is not None:
                log.info("Token expired or expiring soon for %s, refreshing...", email)
            else:
                log.info("Creating new account connection for %s", email)

            # Create fresh account with new token
            account = self._create_account(email)
            self._accounts[email] = CachedAccount(
                account=account,
                created_at=time.time(),
                email=email,
            )
            return account

    def _create_account(self, email: str) -> Account:
        """Create a new authenticated EWS account."""
        token = get_token(email)

        credentials = OAuth2AuthorizationCodeCredentials(
            access_token={"access_token": token, "token_type": "Bearer"}
        )

        config = Configuration(
            server="outlook.office365.com",
            credentials=credentials,
        )

        return Account(
            primary_smtp_address=email,
            config=config,
            autodiscover=False,
            access_type=DELEGATE,
        )

    def refresh_account(self, email: str) -> Account:
        """Force refresh an account's token."""
        with self._lock:
            log.info("Force refreshing account for %s", email)
            account = self._create_account(email)
            self._accounts[email] = CachedAccount(
                account=account,
                created_at=time.time(),
                email=email,
            )
            return account

    def clear_cache(self) -> None:
        """Clear all cached accounts."""
        with self._lock:
            self._accounts.clear()
            log.info("Account cache cleared")

    def get_cache_info(self) -> dict:
        """Get information about cached accounts."""
        with self._lock:
            info = {}
            now = time.time()
            for email, cached in self._accounts.items():
                age = now - cached.created_at
                info[email] = {
                    "age_seconds": age,
                    "is_expired": cached.is_expired(),
                    "time_until_refresh": max(
                        0, cached.token_lifetime - TOKEN_REFRESH_MARGIN_SECONDS - age
                    ),
                }
            return info


# Global account manager instance
_account_manager: Optional[AccountManager] = None
_manager_lock = threading.Lock()


def get_account_manager() -> AccountManager:
    """Get or create the global account manager."""
    global _account_manager
    with _manager_lock:
        if _account_manager is None:
            _account_manager = AccountManager()
        return _account_manager


def get_token(email: str) -> str:
    """Get OAuth2 access token from oama."""
    log.debug("Requesting token for %s from oama", email)
    try:
        result = subprocess.check_output(
            ["oama", "access", email], stderr=subprocess.PIPE
        )
        token = result.decode().strip()
        log.debug("Token obtained successfully for %s", email)
        return token
    except subprocess.CalledProcessError as e:
        log.error(
            "Failed to get token for %s: %s",
            email,
            e.stderr.decode() if e.stderr else str(e),
        )
        raise


def get_account(email: str) -> Account:
    """Get an authenticated EWS Account (with automatic token refresh).

    This function automatically handles token expiry by tracking when
    tokens were created and refreshing them before they expire.
    """
    return get_account_manager().get_account(email)


def refresh_account(email: str) -> Account:
    """Force refresh an account's authentication token."""
    return get_account_manager().refresh_account(email)


def clear_account_cache() -> None:
    """Clear all cached accounts and tokens."""
    get_account_manager().clear_cache()
