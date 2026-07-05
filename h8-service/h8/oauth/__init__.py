"""Integrated OAuth subsystem (replaces the external ``oama`` binary).

This package acquires and persists OAuth tokens in-process:
- :mod:`h8.oauth.store` -- ``TokenStore``: OS keyring with a 0600 file fallback.
- :mod:`h8.oauth.microsoft` -- MSAL device-code login and silent refresh.
- :mod:`h8.oauth.google` -- google-auth loopback / headless URL-paste login.

This module is the facade: it defines the shared :class:`LoginRequired`
exception (imported by both provider modules), the :func:`login_status` /
:func:`logout` helpers, and re-exports the provider entry points.
"""

from typing import Optional


class LoginRequired(Exception):
    """Raised when a valid token cannot be obtained without user interaction."""


# Imports below intentionally follow the LoginRequired definition: the provider
# modules do ``from h8.oauth import LoginRequired`` at import time.
from h8.oauth._account import AccountLike  # noqa: E402
from h8.oauth.store import TokenStore, get_default_store  # noqa: E402
from h8.oauth import google, microsoft  # noqa: E402
from h8.oauth.google import (  # noqa: E402
    LoginSession,
    finish_url_login,
    get_google_credentials,
    poll_login,
    start_login,
)
from h8.oauth.microsoft import (  # noqa: E402
    AccessToken,
    DeviceLogin,
    get_ms_token,
    poll_device_login,
    start_device_login,
)

__all__ = [
    "LoginRequired",
    "AccountLike",
    "TokenStore",
    "get_default_store",
    "AccessToken",
    "DeviceLogin",
    "LoginSession",
    "get_ms_token",
    "start_device_login",
    "poll_device_login",
    "get_google_credentials",
    "start_login",
    "finish_url_login",
    "poll_login",
    "login_status",
    "logout",
]


def login_status(account: AccountLike) -> dict:
    """Return ``{"logged_in", "expires_at", "provider"}`` for ``account``.

    A best-effort silent acquisition determines login state and, where
    available, the token expiry (epoch seconds).
    """
    provider = account.provider
    result: dict[str, object] = {"logged_in": False, "expires_at": None, "provider": provider}

    if provider in ("ews", "graph"):
        resource = "graph" if provider == "graph" else "ews"
        try:
            token = microsoft.get_ms_token(account, resource)  # type: ignore[arg-type]
            result["logged_in"] = True
            result["expires_at"] = token.expires_at
        except LoginRequired:
            pass
        except Exception:  # noqa: BLE001 - status must never raise
            pass
    elif provider == "google":
        try:
            creds = google.get_google_credentials(account)
            result["logged_in"] = True
            result["expires_at"] = creds.expiry.timestamp() if creds.expiry else None
        except LoginRequired:
            pass
        except Exception:  # noqa: BLE001 - status must never raise
            pass

    return result


def logout(account: AccountLike) -> None:
    """Delete any stored OAuth state for ``account``'s provider."""
    provider = account.provider
    if provider in ("ews", "graph"):
        microsoft.delete_cache(account)
    elif provider == "google":
        google.delete_credentials(account)
