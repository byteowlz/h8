"""Shared Google API client plumbing for the Google Workspace backend.

Builds authenticated ``googleapiclient`` service objects from the credentials
managed by :mod:`h8.oauth.google`. All Google backend mixins reach their API
surface through a :class:`GoogleClient` instance at ``self._client``.
"""

from __future__ import annotations

import logging
import threading
from typing import Any, Optional

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
        if self._credentials is None:
            self._credentials = get_google_credentials(self.account)
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
