"""Authentication helpers: oama token acquisition + EWS account compatibility shims.

The exchangelib ``Account`` construction and per-account caching now live in the
EWS backend (``h8/providers/ews``) and the backend registry (``h8/providers/registry``).
This module keeps the oama-based token machinery (Phase-2 replaces it with MSAL)
and thin backward-compatible ``get_account``/``refresh_account`` shims that return
the exchangelib ``Account`` owned by the EWS backend -- used by the legacy direct
CLI in ``h8/cli.py``.

Deliberately does NOT import exchangelib at module scope: account construction is
delegated to the EWS backend so non-EWS callers do not pay the import cost.
"""

import logging
import os
import platform
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING, Optional
from urllib.request import urlopen

if TYPE_CHECKING:  # pragma: no cover - typing only
    from exchangelib import Account

log = logging.getLogger(__name__)

# oama GitHub release info
OAMA_REPO = "pdobsan/oama"


def ensure_gpg_headless() -> None:
    """Configure GPG for headless (non-interactive) operation.

    On systems without a display (headless servers), GPG's pinentry can hang
    waiting for a GUI/TUI that will never appear. This function:
    1. Sets GPG_TTY if a tty is available
    2. Enables loopback pinentry mode in gpg-agent.conf
    3. Sets pinentry-mode loopback in gpg.conf
    4. Restarts gpg-agent to pick up changes

    Safe to call multiple times -- only modifies config if needed.
    """
    import platform

    # On macOS, pinentry-mac handles GUI/headless correctly -- skip loopback config
    # which would kill the agent (dropping cached passphrases) and cause plain-text
    # passphrase prompts to bleed into the terminal
    if platform.system() == "Darwin":
        log.debug("macOS detected, skipping headless GPG setup (pinentry-mac handles it)")
        return

    display = os.environ.get("DISPLAY", "")
    wayland = os.environ.get("WAYLAND_DISPLAY", "")

    # Only apply headless fixes when no display server is available
    if display or wayland:
        log.debug("Display detected (%s), skipping headless GPG setup", display or wayland)
        return

    log.info("No display detected, configuring GPG for headless operation")

    # Set GPG_TTY
    try:
        tty = subprocess.check_output(["tty"], stderr=subprocess.PIPE, timeout=5).decode().strip()
        if tty and "not a tty" not in tty:
            os.environ["GPG_TTY"] = tty
            log.debug("Set GPG_TTY=%s", tty)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        log.debug("No tty available, GPG_TTY not set")

    gnupg_dir = Path.home() / ".gnupg"
    gnupg_dir.mkdir(mode=0o700, exist_ok=True)

    # Enable loopback pinentry in gpg-agent.conf
    agent_conf = gnupg_dir / "gpg-agent.conf"
    _ensure_config_line(agent_conf, "allow-loopback-pinentry")

    # Set pinentry-mode loopback in gpg.conf
    gpg_conf = gnupg_dir / "gpg.conf"
    _ensure_config_line(gpg_conf, "pinentry-mode loopback")

    # Restart gpg-agent to pick up changes
    try:
        subprocess.run(
            ["gpgconf", "--kill", "gpg-agent"],
            capture_output=True, timeout=10,
        )
        log.debug("Restarted gpg-agent")
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, FileNotFoundError):
        log.debug("Could not restart gpg-agent")


def _ensure_config_line(path: Path, line: str) -> None:
    """Add a line to a config file if not already present."""
    if path.exists():
        content = path.read_text()
        if line in content:
            return
        if not content.endswith("\n"):
            content += "\n"
        content += line + "\n"
    else:
        content = line + "\n"
    path.write_text(content)
    log.debug("Added '%s' to %s", line, path)
OAMA_INSTALL_DIR = Path.home() / ".local" / "bin"


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
    """Ensure oama is installed and GPG is configured for the environment."""
    ensure_gpg_headless()

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
        subprocess.check_output(
            ["oama", "renew", email], stderr=subprocess.PIPE, timeout=30
        )
        log.info("Token renewed successfully for %s", email)
        return True
    except subprocess.TimeoutExpired:
        log.error("Token renewal timed out for %s", email)
        return False
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
            ["oama", "access", email], stderr=subprocess.PIPE, timeout=10
        )
        token = result.decode().strip()
        log.debug("Token obtained successfully for %s", email)
        return token
    except subprocess.TimeoutExpired:
        log.error("Token access timed out for %s", email)
        if attempt_renew:
            log.info("Attempting token renewal for %s after timeout", email)
            if renew_token(email):
                return get_token(email, attempt_renew=False)
        raise RuntimeError(f"Token access timed out for {email}")
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


def get_account(email: str) -> "Account":
    """Return the exchangelib ``Account`` for ``email`` (backward-compat shim).

    Delegates to the EWS backend owned by the registry, which builds and caches
    the exchangelib ``Account``. Used by the legacy direct CLI in ``h8/cli.py``.
    """
    from h8.providers.base import PROVIDER_EWS
    from h8.providers.registry import get_backend

    backend = get_backend(email)
    if backend.provider != PROVIDER_EWS:
        raise RuntimeError(
            f"account '{email}' is not an EWS account "
            f"(provider '{backend.provider}'); the direct exchangelib CLI only "
            "supports EWS accounts"
        )
    return backend.ews_account


def refresh_account(email: str) -> "Account":
    """Force a fresh token + exchangelib ``Account`` (backward-compat shim)."""
    from h8.providers.base import PROVIDER_EWS
    from h8.providers.registry import get_backend

    backend = get_backend(email)
    if backend.provider != PROVIDER_EWS:
        raise RuntimeError(f"account '{email}' is not an EWS account")
    backend.refresh()
    return backend.ews_account


def renew_and_refresh_account(email: str) -> "Account":
    """Renew the oama token, then rebuild the account (backward-compat shim)."""
    log.info("Renewing and refreshing account for %s", email)
    renew_token(email)
    return refresh_account(email)


def clear_account_cache() -> None:
    """Clear all cached backends/accounts."""
    from h8.providers.registry import clear_cache

    clear_cache()
