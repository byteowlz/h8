"""Google Workspace backend: Gmail, Google Calendar, People API.

Composition of per-domain mixins (implementation in sibling modules):

- :mod:`h8.providers.google.mail`      -- ``GoogleMailMixin``
- :mod:`h8.providers.google.calendar`  -- ``GoogleCalendarMixin`` (events + freebusy)
- :mod:`h8.providers.google.contacts`  -- ``GoogleContactsMixin``
- :mod:`h8.providers.google.settings`  -- ``GoogleSettingsMixin`` (OOF / vacation)

Each mixin implements a subset of the ``Backend`` domain methods (see
``h8/providers/base.py`` for the frozen contract) using ``self._client``
(:class:`h8.providers.google.client.GoogleClient`) and ``self.account``.
Methods outside the advertised capabilities are left unimplemented; routes
gate on ``capabilities`` and return 501.

Response shapes are FROZEN to the existing EWS-derived JSON: ``id`` carries
the Gmail message id / Calendar event id / People resourceName, ``changekey``
is ``null`` (except contacts, where it carries the People API ``etag``).
"""

from __future__ import annotations

from h8.providers.base import (
    CAP_CALENDAR,
    CAP_CALENDAR_MEETINGS,
    CAP_CONTACTS,
    CAP_FREEBUSY_OTHERS,
    CAP_FREEBUSY_SELF,
    CAP_MAIL,
    CAP_MAIL_FOLDERS,
    CAP_MAIL_SEND,
    CAP_OOF,
    PROVIDER_GOOGLE,
    AccountConfig,
    Backend,
)
from h8.providers.google.calendar import GoogleCalendarMixin
from h8.providers.google.client import GoogleClient
from h8.providers.google.contacts import GoogleContactsMixin
from h8.providers.google.mail import GoogleMailMixin
from h8.providers.google.settings import GoogleSettingsMixin

GOOGLE_CAPABILITIES = frozenset(
    {
        CAP_MAIL,
        CAP_MAIL_SEND,
        CAP_MAIL_FOLDERS,
        CAP_CALENDAR,
        CAP_CALENDAR_MEETINGS,
        CAP_CONTACTS,
        CAP_FREEBUSY_SELF,
        CAP_FREEBUSY_OTHERS,
        CAP_OOF,
    }
)


class GoogleBackend(
    GoogleMailMixin,
    GoogleCalendarMixin,
    GoogleContactsMixin,
    GoogleSettingsMixin,
    Backend,
):
    """Google Workspace implementation of the h8 backend contract."""

    provider = PROVIDER_GOOGLE
    capabilities = GOOGLE_CAPABILITIES

    def __init__(self, account: AccountConfig) -> None:
        self.account = account
        self._client = GoogleClient(account)

    def refresh(self) -> None:
        """Drop cached Google services/credentials; next call re-authenticates."""
        self._client.invalidate()
