"""Backend registry: resolve an account reference to a cached :class:`Backend`.

Replaces the old ``auth.AccountManager`` pattern. Backends are cached per account
reference (alias or email) and constructed under a lock so concurrent requests
for the same account share one instance.

Provider implementation packages are imported lazily inside the factory
functions so that importing this module (or ``providers.base``) never pulls in a
provider SDK such as exchangelib.
"""

from __future__ import annotations

import logging
import threading
import time
from typing import Callable, Dict

from h8.accounts import resolve_account
from h8.providers.base import (
    PROVIDER_EWS,
    PROVIDER_GOOGLE,
    AccountConfig,
    Backend,
    BackendNotSupported,
)

log = logging.getLogger(__name__)


def _make_ews(account: AccountConfig) -> Backend:
    from h8.providers.ews import EwsBackend

    return EwsBackend(account)


def _make_google(account: AccountConfig) -> Backend:
    from h8.providers.google import GoogleBackend

    return GoogleBackend(account)


#: provider id -> factory. Phase-2 providers register here.
_FACTORIES: Dict[str, Callable[[AccountConfig], Backend]] = {
    PROVIDER_EWS: _make_ews,
    PROVIDER_GOOGLE: _make_google,
}

_cache: Dict[str, Backend] = {}
_lock = threading.Lock()


def get_backend(account_ref: str | None) -> Backend:
    """Resolve ``account_ref`` and return a cached backend for it.

    Args:
        account_ref: An alias, a bare email, or ``None`` for the default account.

    Returns:
        A :class:`Backend` instance (constructed once per account reference).

    Raises:
        AccountResolutionError: If the reference cannot be resolved.
        BackendNotSupported: If no provider is registered for the account, or the
            provider is not yet implemented.

    Note:
        Backend construction and any token acquisition MUST run off the event
        loop. Callers in ``h8/service`` invoke this inside the threadpool
        wrappers (``safe_call``/``safe_call_with_retry``) or ``run_in_threadpool``.
    """
    account = resolve_account(account_ref)
    key = account.ref

    with _lock:
        backend = _cache.get(key)
        if backend is not None:
            return backend

        factory = _FACTORIES.get(account.provider)
        if factory is None:
            raise BackendNotSupported(
                f"no provider registered for '{account.provider}' "
                f"(account '{key}')"
            )
        log.info(
            "Constructing %s backend for account '%s'", account.provider, key
        )
        backend = factory(account)
        _cache[key] = backend
        return backend


def clear_cache() -> None:
    """Drop all cached backends."""
    with _lock:
        _cache.clear()
        log.info("Backend cache cleared")


def get_cache_info() -> dict:
    """Return diagnostic info about cached backends (used by ``/health``)."""
    with _lock:
        now = time.time()
        info: dict = {}
        for key, backend in _cache.items():
            created_at = getattr(backend, "created_at", 0.0) or 0.0
            info[key] = {
                "provider": backend.provider,
                "email": backend.account.email,
                "age_seconds": (now - created_at) if created_at else None,
            }
        return info
