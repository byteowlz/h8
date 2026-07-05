"""Shared Google API client plumbing for the Google Workspace backend.

Builds authenticated ``googleapiclient`` service objects from the credentials
managed by :mod:`h8.oauth.google`. All Google backend mixins reach their API
surface through a :class:`GoogleClient` instance at ``self._client``.
"""

from __future__ import annotations

import logging
import threading
from typing import Any, Optional

from h8.auth import AuthLoginRequired
from h8.oauth import LoginRequired  # noqa: F401  (re-exported for mixins)
from h8.oauth.google import get_google_credentials
from h8.providers.base import AccountConfig

log = logging.getLogger(__name__)


class GoogleClient:
    """Lazily-built, cached Google API services for one account.

    Thread-safe: service construction is guarded by a lock. ``invalidate()``
    drops all cached services and credentials so the next call re-authenticates
    (used by ``Backend.refresh()``).
    """

    def __init__(self, account: AccountConfig) -> None:
        self.account = account
        self._lock = threading.Lock()
        self._services: dict[str, Any] = {}
        self._credentials: Optional[Any] = None

    def _get_credentials(self) -> Any:
        """Return cached Google credentials, acquiring them on first use.

        Translates the OAuth layer's :class:`~h8.oauth.LoginRequired` into
        :class:`~h8.auth.AuthLoginRequired` (a ``BackendAuthError`` subclass the
        service maps straight to HTTP 401 with an actionable ``h8 auth login``
        message and no futile refresh+retry), mirroring the EWS backend. This is
        the single chokepoint every service builder (``gmail``/``calendar``/
        ``people``) passes through, so all Google mixins get the same behavior.
        """
        if self._credentials is None:
            ref = self.account.ref
            try:
                self._credentials = get_google_credentials(self.account)
            except LoginRequired as exc:
                raise AuthLoginRequired(
                    f"Google login required for account '{ref}'. "
                    f"Run `h8 auth login {ref}` to sign in. ({exc})"
                ) from exc
        return self._credentials

    def _service(self, name: str, version: str) -> Any:
        key = f"{name}:{version}"
        with self._lock:
            svc = self._services.get(key)
            if svc is None:
                from googleapiclient.discovery import build

                svc = build(
                    name,
                    version,
                    credentials=self._get_credentials(),
                    cache_discovery=False,
                )
                self._services[key] = svc
            return svc

    def gmail(self) -> Any:
        """Gmail API v1 service (``users.messages``, ``users.drafts``, ...)."""
        return self._service("gmail", "v1")

    def calendar(self) -> Any:
        """Calendar API v3 service (``events``, ``freebusy``)."""
        return self._service("calendar", "v3")

    def people(self) -> Any:
        """People API v1 service (``people.connections``)."""
        return self._service("people", "v1")

    def invalidate(self) -> None:
        """Drop cached services and credentials; next use re-authenticates."""
        with self._lock:
            self._services.clear()
            self._credentials = None
