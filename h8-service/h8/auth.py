"""Authentication compatibility shims for the EWS account surface.

The exchangelib ``Account`` construction and per-account caching live in the EWS
backend (``h8/providers/ews``) and the backend registry
(``h8/providers/registry``). OAuth token acquisition lives in ``h8/oauth`` (MSAL
for Microsoft, google-auth for Google) -- the external ``oama`` binary and all
GPG/pinentry machinery it required have been removed.

What remains here:
- :class:`AuthLoginRequired` -- a ``BackendAuthError`` subclass raised when the
  OAuth layer needs an interactive login. It is distinct from a stale-token
  ``BackendAuthError`` so the service returns HTTP 401 (telling the user to run
  ``h8 auth login <account>``) instead of a pointless refresh+retry.
- Thin ``get_account`` / ``refresh_account`` shims that return the exchangelib
  ``Account`` owned by the EWS backend -- used by the legacy direct CLI in
  ``h8/cli.py``.

Deliberately does NOT import exchangelib at module scope: account construction is
delegated to the EWS backend so non-EWS callers do not pay the import cost.
"""

import logging
from typing import TYPE_CHECKING

from h8.providers.base import BackendAuthError

if TYPE_CHECKING:  # pragma: no cover - typing only
    from exchangelib import Account

log = logging.getLogger(__name__)


class AuthLoginRequired(BackendAuthError):
    """Interactive OAuth login is required for this account.

    Raised by the EWS backend when the OAuth layer signals
    :class:`h8.oauth.LoginRequired` (no cached credentials or silent refresh
    failed). A ``BackendAuthError`` subclass so it belongs to the same family,
    but the service layer catches it *before* the generic ``BackendAuthError``
    handler and maps it straight to HTTP 401 -- refresh+retry cannot help when
    the user has never logged in.
    """


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


def clear_account_cache() -> None:
    """Clear all cached backends/accounts."""
    from h8.providers.registry import clear_cache

    clear_cache()
