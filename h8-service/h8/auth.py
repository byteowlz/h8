"""Authentication and EWS account connection with automatic token refresh."""

import logging
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional
from urllib.request import urlopen

from exchangelib import Account, Configuration, DELEGATE
from exchangelib import OAuth2AuthorizationCodeCredentials

log = logging.getLogger(__name__)

# oama GitHub release info
OAMA_REPO = "pdobsan/oama"
OAMA_INSTALL_DIR = Path.home() / ".local" / "bin"

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


def is_oama_installed() -> bool:
    """Check if oama is available in PATH."""
    return shutil.which("oama") is not None


def get_oama_platform_suffix() -> str:
    """Get the platform suffix for oama binary download."""
    system = platform.system()
    machine = platform.machine().lower()

    if system == "Darwin":
        arch = "arm64" if machine in ("arm64", "aarch64") else "x86_64"
        return f"Darwin-{arch}"
    elif system == "Linux":
        arch = "aarch64" if machine in ("arm64", "aarch64") else "x86_64"
        return f"Linux-{arch}"
    else:
        raise RuntimeError(f"Unsupported platform: {system} {machine}")


def get_latest_oama_version() -> str:
    """Fetch the latest oama release version from GitHub."""
    import json

    url = f"https://api.github.com/repos/{OAMA_REPO}/releases/latest"
    with urlopen(url, timeout=30) as resp:
        data = json.loads(resp.read().decode())
        return data["tag_name"]


def install_oama(version: Optional[str] = None) -> Path:
    """Download and install oama binary.

    Args:
        version: Specific version to install (e.g., "0.22.0"), or None for latest.

    Returns:
        Path to the installed oama binary.

    Raises:
        RuntimeError: If installation fails.
    """
    if version is None:
        version = get_latest_oama_version()

    platform_suffix = get_oama_platform_suffix()
    tarball_name = f"oama-{version}-{platform_suffix}.tar.gz"
    download_url = (
        f"https://github.com/{OAMA_REPO}/releases/download/{version}/{tarball_name}"
    )

    log.info("Downloading oama %s from %s", version, download_url)

    # Create install directory if needed
    OAMA_INSTALL_DIR.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as tmpdir:
        tarball_path = Path(tmpdir) / tarball_name

        # Download the tarball
        with urlopen(download_url, timeout=60) as resp:
            tarball_path.write_bytes(resp.read())

        # Extract the binary
        with tarfile.open(tarball_path, "r:gz") as tar:
            # Find the oama binary in the archive
            for member in tar.getmembers():
                if member.name.endswith("/oama") or member.name == "oama":
                    # Extract to temp dir first
                    tar.extract(member, tmpdir)
                    extracted_path = Path(tmpdir) / member.name
                    break
            else:
                raise RuntimeError("oama binary not found in tarball")

        # Move to install location
        install_path = OAMA_INSTALL_DIR / "oama"
        shutil.move(str(extracted_path), str(install_path))
        install_path.chmod(0o755)

        log.info("Installed oama to %s", install_path)

    # Verify installation
    if not is_oama_installed():
        # Add to PATH hint
        log.warning(
            "oama installed to %s but not in PATH. Add %s to your PATH.",
            install_path,
            OAMA_INSTALL_DIR,
        )
        # Update PATH for current process
        os.environ["PATH"] = f"{OAMA_INSTALL_DIR}:{os.environ.get('PATH', '')}"

    return install_path


def ensure_oama() -> None:
    """Ensure oama is installed, installing it if necessary."""
    if is_oama_installed():
        return

    log.warning("oama not found in PATH, attempting to install...")
    try:
        install_oama()
        log.info("oama installed successfully")
    except Exception as e:
        log.error("Failed to install oama: %s", e)
        raise RuntimeError(
            "oama is required but not installed. "
            "Install it manually from https://github.com/pdobsan/oama/releases "
            f"or add {OAMA_INSTALL_DIR} to your PATH if already installed."
        ) from e


def renew_token(email: str) -> bool:
    """Renew OAuth2 token via oama renew.

    Returns True if renewal succeeded, False otherwise.
    """
    ensure_oama()
    log.info("Renewing token for %s via oama renew", email)
    try:
        subprocess.check_output(["oama", "renew", email], stderr=subprocess.PIPE)
        log.info("Token renewed successfully for %s", email)
        return True
    except subprocess.CalledProcessError as e:
        log.warning(
            "Failed to renew token for %s: %s",
            email,
            e.stderr.decode() if e.stderr else str(e),
        )
        return False


def get_token(email: str, attempt_renew: bool = True) -> str:
    """Get OAuth2 access token from oama.

    If access fails and attempt_renew is True, tries oama renew first.
    """
    ensure_oama()
    log.debug("Requesting token for %s from oama", email)
    try:
        result = subprocess.check_output(
            ["oama", "access", email], stderr=subprocess.PIPE
        )
        token = result.decode().strip()
        log.debug("Token obtained successfully for %s", email)
        return token
    except subprocess.CalledProcessError as e:
        stderr_msg = e.stderr.decode() if e.stderr else str(e)
        log.warning("Failed to get token for %s: %s", email, stderr_msg)

        # Try renewing if this is the first attempt
        if attempt_renew:
            log.info("Attempting token renewal for %s", email)
            if renew_token(email):
                # Retry access after successful renewal
                return get_token(email, attempt_renew=False)

        log.error("Token retrieval failed for %s after renewal attempt", email)
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


def renew_and_refresh_account(email: str) -> Account:
    """Renew token via oama and then refresh the account.

    Use this when UnauthorizedError occurs - it explicitly calls oama renew
    before getting a fresh token.
    """
    log.info("Renewing and refreshing account for %s", email)
    renew_token(email)
    return get_account_manager().refresh_account(email)


def clear_account_cache() -> None:
    """Clear all cached accounts and tokens."""
    get_account_manager().clear_cache()
