"""Provider abstraction: capabilities, exceptions, config, and domain contracts.

This module is the **authoritative contract** for h8 backends. Every provider
(EWS today, Google/Graph later) implements :class:`Backend`, which is composed of
per-domain mixins. The method signatures mirror the module functions that the
FastAPI routes in ``h8/service/__init__.py`` call today, minus the leading
``(account)`` positional argument -- the backend instance carries the account.

Design rules:
- This module MUST NOT import ``exchangelib`` (or any provider SDK). Keep those
  imports inside ``providers/ews`` and the legacy ``h8/*.py`` modules so that
  future backends (and callers that only need ``base``) do not pay the import
  cost.
- Domain methods have concrete default bodies that raise ``NotImplementedError``.
  A backend advertises what it actually supports through :attr:`Backend.capabilities`;
  routes gate on capabilities and return HTTP 501 before ever calling an
  unsupported method. This lets a backend partially implement the surface.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

# ---------------------------------------------------------------------------
# Provider identifiers
# ---------------------------------------------------------------------------

PROVIDER_EWS = "ews"
PROVIDER_GOOGLE = "google"
PROVIDER_GRAPH = "graph"

# ---------------------------------------------------------------------------
# Capability constants
# ---------------------------------------------------------------------------

CAP_MAIL = "mail"
CAP_MAIL_SEND = "mail:send"
CAP_MAIL_FOLDERS = "mail:folders"
CAP_MAIL_SCHEDULED_SEND = "mail:scheduled_send"
CAP_CALENDAR = "calendar"
CAP_CALENDAR_MEETINGS = "calendar:meetings"
CAP_CONTACTS = "contacts"
CAP_FREEBUSY_SELF = "freebusy:self"
CAP_FREEBUSY_OTHERS = "freebusy:others"
CAP_GAL = "gal"
CAP_RESOURCES = "resources"
CAP_RULES = "rules"
CAP_OOF = "oof"
CAP_BOOKINGS = "bookings"

#: Every capability EWS advertises (the full h8 surface).
ALL_CAPABILITIES = frozenset(
    {
        CAP_MAIL,
        CAP_MAIL_SEND,
        CAP_MAIL_FOLDERS,
        CAP_MAIL_SCHEDULED_SEND,
        CAP_CALENDAR,
        CAP_CALENDAR_MEETINGS,
        CAP_CONTACTS,
        CAP_FREEBUSY_SELF,
        CAP_FREEBUSY_OTHERS,
        CAP_GAL,
        CAP_RESOURCES,
        CAP_RULES,
        CAP_OOF,
        CAP_BOOKINGS,
    }
)


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class BackendError(Exception):
    """Base class for all backend-originated errors."""


class BackendAuthError(BackendError):
    """Authentication/authorization failed.

    Signals that credentials are stale or invalid. The service layer responds
    by calling :meth:`Backend.refresh` and retrying the call exactly once.
    """


class BackendBusyError(BackendError):
    """The upstream provider is throttling or temporarily unavailable.

    ``retry_after`` is the server-suggested delay in seconds, if known. The
    service layer backs off (using ``retry_after`` when present) and retries.
    """

    def __init__(self, message: str = "", retry_after: Optional[float] = None) -> None:
        super().__init__(message)
        self.retry_after = retry_after


class BackendNotSupported(BackendError):
    """The requested capability is not supported by this backend.

    ``capability`` names the missing capability (one of the ``CAP_*`` constants),
    or ``None`` when the whole backend is unavailable (e.g. a not-yet-implemented
    provider). The service layer maps this to HTTP 501.
    """

    def __init__(self, message: str = "", capability: Optional[str] = None) -> None:
        super().__init__(message)
        self.capability = capability


# ---------------------------------------------------------------------------
# Account configuration
# ---------------------------------------------------------------------------


@dataclass
class AccountConfig:
    """Resolved account descriptor produced by ``h8/accounts.py``.

    Attributes:
        alias: The ``[accounts.<alias>]`` key, or ``None`` for an account that
            was referenced by bare email (legacy / implicit EWS).
        email: The mailbox address.
        provider: Provider id -- ``"ews"``, ``"google"`` or ``"graph"``.
        client_id: OAuth client id (provider-specific; may be ``None`` to use a
            built-in default).
        tenant: MSAL authority tenant (Microsoft only), default ``"organizations"``.
        extra: Any additional provider options from the config table
            (e.g. ``client_secret`` for Google).
    """

    email: str
    provider: str = PROVIDER_EWS
    alias: Optional[str] = None
    client_id: Optional[str] = None
    tenant: str = "organizations"
    extra: Dict[str, Any] = field(default_factory=dict)

    @property
    def ref(self) -> str:
        """The canonical reference for this account: alias if set, else email."""
        return self.alias or self.email


# ---------------------------------------------------------------------------
# Domain contracts
#
# Each method mirrors the corresponding ``h8/<module>.py`` function, minus the
# leading ``account`` argument. Signatures below are FROZEN: Phase-2 backends
# implement against exactly these names/params, and the FastAPI routes call
# exactly these methods.
# ---------------------------------------------------------------------------


class MailBackend(ABC):
    """Mailbox operations (maps to ``h8/mail.py`` and ``h8/unsubscribe.py``)."""

    def list_messages(
        self, folder: str = "inbox", limit: int = 20, unread: bool = False
    ) -> List[dict]:
        """List messages in ``folder`` (newest first). Mirrors ``mail.list_messages``."""
        raise NotImplementedError

    def search_messages(
        self,
        query: str,
        folder: str = "inbox",
        limit: int = 50,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> List[dict]:
        """Search messages by subject/sender/body. Mirrors ``mail.search_messages``."""
        raise NotImplementedError

    def get_message(self, item_id: str, folder: str = "inbox") -> dict:
        """Fetch a single message. Mirrors ``mail.get_message``."""
        raise NotImplementedError

    def batch_get_messages(
        self, item_ids: List[str], folder: str = "inbox"
    ) -> List[dict]:
        """Fetch several messages by id in one round trip. Mirrors ``mail.batch_get_messages``."""
        raise NotImplementedError

    def send_message(self, message_data: dict) -> dict:
        """Send (or schedule) a message. Mirrors ``mail.send_message``."""
        raise NotImplementedError

    def fetch_messages(
        self,
        folder: str,
        output_dir: str,
        format: str = "maildir",
        limit: Optional[int] = None,
    ) -> dict:
        """Export messages to maildir/mbox. Mirrors ``mail.fetch_messages``."""
        raise NotImplementedError

    def save_draft(self, draft_data: dict) -> dict:
        """Create a draft. Mirrors ``mail.save_draft``."""
        raise NotImplementedError

    def update_draft(self, item_id: str, update_data: dict) -> dict:
        """Update an existing draft. Mirrors ``mail.update_draft``."""
        raise NotImplementedError

    def delete_draft(self, item_id: str) -> dict:
        """Delete a draft. Mirrors ``mail.delete_draft``."""
        raise NotImplementedError

    def list_attachments(self, item_id: str, folder: str = "inbox") -> List[dict]:
        """List a message's attachments. Mirrors ``mail.list_attachments``."""
        raise NotImplementedError

    def download_attachment(
        self,
        item_id: str,
        attachment_index: int,
        output_path: str,
        folder: str = "inbox",
    ) -> dict:
        """Download one attachment to disk. Mirrors ``mail.download_attachment``."""
        raise NotImplementedError

    def delete_message(
        self, item_id: str, folder: str = "inbox", permanent: bool = False
    ) -> dict:
        """Delete a message (trash or permanent). Mirrors ``mail.delete_message``."""
        raise NotImplementedError

    def move_message(
        self,
        item_id: str,
        target_folder: str,
        source_folder: str = "inbox",
        create_folder: bool = False,
    ) -> dict:
        """Move a message to another folder. Mirrors ``mail.move_message``."""
        raise NotImplementedError

    def empty_folder(self, folder_name: str = "trash") -> dict:
        """Permanently empty a folder. Mirrors ``mail.empty_folder``."""
        raise NotImplementedError

    def batch_move_messages(
        self,
        folder: str,
        target_folder: str,
        older_than_days: int,
        query: Optional[str] = None,
        limit: int = 500,
        create_folder: bool = True,
        dry_run: bool = False,
    ) -> dict:
        """Bulk-move messages by age/query. Mirrors ``mail.batch_move_messages``."""
        raise NotImplementedError

    def batch_mark_messages(
        self,
        folder: str,
        read: bool,
        ids: Optional[List[str]] = None,
        older_than_days: Optional[int] = None,
        query: Optional[str] = None,
        limit: int = 500,
        dry_run: bool = False,
    ) -> dict:
        """Bulk mark read/unread. Mirrors ``mail.batch_mark_messages``."""
        raise NotImplementedError

    def mark_as_spam(
        self, item_id: str, is_spam: bool = True, move_to_junk: bool = True
    ) -> dict:
        """Mark a message as spam/not spam. Mirrors ``mail.mark_as_spam``."""
        raise NotImplementedError

    def scan_unsubscribe(
        self,
        folder: str = "inbox",
        sender: Optional[str] = None,
        search: Optional[str] = None,
        limit: int = 50,
        safe_senders: Optional[List[str]] = None,
        blocked_patterns: Optional[List[str]] = None,
    ) -> List[dict]:
        """Scan messages for unsubscribe links (dry run). Mirrors ``unsubscribe.scan_messages``."""
        raise NotImplementedError

    def execute_unsubscribe(
        self,
        item_ids: List[str],
        safe_senders: Optional[List[str]] = None,
        blocked_patterns: Optional[List[str]] = None,
        trusted_domains: Optional[List[str]] = None,
        rate_limit_seconds: float = 2.0,
    ) -> List[dict]:
        """Visit unsubscribe links for the given messages. Mirrors ``unsubscribe.execute_unsubscribe``."""
        raise NotImplementedError


class CalendarBackend(ABC):
    """Calendar operations (maps to ``h8/calendar.py``)."""

    def list_events(
        self,
        days: int = 7,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> List[dict]:
        """List events in a view window. Mirrors ``calendar.list_events``."""
        raise NotImplementedError

    def get_event(self, item_id: str) -> dict:
        """Fetch one event. Mirrors ``calendar.get_event``."""
        raise NotImplementedError

    def create_event(self, event_data: dict) -> dict:
        """Create an event (no attendees). Mirrors ``calendar.create_event``."""
        raise NotImplementedError

    def invite_event(self, event_data: dict) -> dict:
        """Create an event and send meeting invites. Mirrors ``calendar.invite_event``."""
        raise NotImplementedError

    def delete_event(self, item_id: str) -> dict:
        """Delete an event. Mirrors ``calendar.delete_event``."""
        raise NotImplementedError

    def cancel_event(self, item_id: str, message: Optional[str] = None) -> dict:
        """Cancel a meeting and notify attendees. Mirrors ``calendar.cancel_event``."""
        raise NotImplementedError

    def search_events(
        self,
        query: str,
        days: int = 90,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
        limit: int = 50,
    ) -> List[dict]:
        """Search events. Mirrors ``calendar.search_events``."""
        raise NotImplementedError

    def list_invites(self, limit: int = 50) -> List[dict]:
        """List pending meeting requests. Mirrors ``calendar.list_invites``."""
        raise NotImplementedError

    def rsvp_event(
        self, item_id: str, response: str, message: Optional[str] = None
    ) -> dict:
        """Respond to a meeting invite. Mirrors ``calendar.rsvp_event``."""
        raise NotImplementedError


class ContactsBackend(ABC):
    """Contact operations (maps to ``h8/contacts.py``)."""

    def list_contacts(
        self, limit: int = 100, search: Optional[str] = None
    ) -> List[dict]:
        """List/search contacts. Mirrors ``contacts.list_contacts``."""
        raise NotImplementedError

    def get_contact(self, item_id: str) -> dict:
        """Fetch one contact. Mirrors ``contacts.get_contact``."""
        raise NotImplementedError

    def create_contact(self, contact_data: dict) -> dict:
        """Create a contact. Mirrors ``contacts.create_contact``."""
        raise NotImplementedError

    def delete_contact(self, item_id: str) -> dict:
        """Delete a contact. Mirrors ``contacts.delete_contact``."""
        raise NotImplementedError

    def update_contact(self, item_id: str, updates: dict) -> dict:
        """Update a contact. Mirrors ``contacts.update_contact``."""
        raise NotImplementedError


class AvailabilityBackend(ABC):
    """Free/busy and resource availability (maps to ``h8/free.py``, ``h8/people.py``,
    ``h8/resources.py``).

    Note: resource availability (``CAP_RESOURCES``) lives here alongside self/others
    free-busy; each method is gated by its own capability at the route layer.
    """

    def find_free_slots(
        self,
        weeks: int = 1,
        duration_minutes: int = 30,
        limit: Optional[int] = None,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
        exclude_weekends: Optional[bool] = None,
    ) -> List[dict]:
        """Own free slots. Mirrors ``free.find_free_slots``."""
        raise NotImplementedError

    def get_person_agenda(
        self,
        email: str,
        days: int = 7,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> List[dict]:
        """Another person's busy times. Mirrors ``people.get_person_agenda``."""
        raise NotImplementedError

    def get_person_free_slots(
        self,
        email: str,
        weeks: int = 1,
        duration_minutes: int = 30,
        limit: Optional[int] = None,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
        exclude_weekends: Optional[bool] = None,
    ) -> List[dict]:
        """Another person's free slots. Mirrors ``people.get_person_free_slots``."""
        raise NotImplementedError

    def find_common_free_slots(
        self,
        emails: List[str],
        weeks: int = 1,
        duration_minutes: int = 30,
        limit: Optional[int] = None,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
        exclude_weekends: Optional[bool] = None,
    ) -> List[dict]:
        """Common free slots across people. Mirrors ``people.find_common_free_slots``."""
        raise NotImplementedError

    def resource_free(
        self,
        resources: List[dict],
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
        days: int = 1,
        start_hour: Optional[int] = None,
        end_hour: Optional[int] = None,
    ) -> List[dict]:
        """Free slots per resource. Mirrors ``resources.resource_free``."""
        raise NotImplementedError

    def resource_free_window(
        self, resources: List[dict], from_date: str, to_date: str
    ) -> List[dict]:
        """Resource availability in a window. Mirrors ``resources.resource_free_window``."""
        raise NotImplementedError

    def resource_agenda(
        self,
        resources: List[dict],
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
        days: int = 1,
    ) -> List[dict]:
        """Bookings per resource. Mirrors ``resources.resource_agenda``."""
        raise NotImplementedError


class DirectoryBackend(ABC):
    """Global address list / name resolution (maps to ``h8/resolve.py``)."""

    def resolve_names(self, query: str) -> List[dict]:
        """Resolve names against the GAL. Mirrors ``resolve.resolve_names``."""
        raise NotImplementedError

    def validate_email(self, email: str) -> bool:
        """Check that an address resolves to a real mailbox. Mirrors ``resolve.validate_email``."""
        raise NotImplementedError


class SettingsBackend(ABC):
    """Inbox rules and Out-of-Office (maps to ``h8/rules_oof.py``)."""

    def list_rules(self) -> List[Dict[str, Any]]:
        """List inbox rules. Mirrors ``rules_oof.list_rules``."""
        raise NotImplementedError

    def get_rule(self, rule_id: str) -> Optional[Dict[str, Any]]:
        """Fetch one rule. Mirrors ``rules_oof.get_rule``."""
        raise NotImplementedError

    def create_rule(
        self,
        display_name: str,
        priority: int = 1,
        is_enabled: bool = True,
        conditions: Optional[Dict[str, Any]] = None,
        actions: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        """Create an inbox rule. Mirrors ``rules_oof.create_rule``."""
        raise NotImplementedError

    def update_rule(
        self,
        rule_id: str,
        display_name: Optional[str] = None,
        priority: Optional[int] = None,
        is_enabled: Optional[bool] = None,
        conditions: Optional[Dict[str, Any]] = None,
        actions: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        """Update an inbox rule. Mirrors ``rules_oof.update_rule``."""
        raise NotImplementedError

    def enable_rule(self, rule_id: str) -> Dict[str, Any]:
        """Enable a rule. Mirrors ``rules_oof.enable_rule``."""
        raise NotImplementedError

    def disable_rule(self, rule_id: str) -> Dict[str, Any]:
        """Disable a rule. Mirrors ``rules_oof.disable_rule``."""
        raise NotImplementedError

    def delete_rule(self, rule_id: str) -> None:
        """Delete a rule. Mirrors ``rules_oof.delete_rule``."""
        raise NotImplementedError

    def get_oof_settings(self) -> Dict[str, Any]:
        """Get Out-of-Office settings. Mirrors ``rules_oof.get_oof_settings``."""
        raise NotImplementedError

    def set_oof_settings(
        self,
        state: str,
        external_audience: Optional[str] = None,
        start: Optional[str] = None,
        end: Optional[str] = None,
        internal_reply: Optional[str] = None,
        external_reply: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Set Out-of-Office settings. Mirrors ``rules_oof.set_oof_settings``."""
        raise NotImplementedError

    def enable_oof(
        self,
        internal_reply: str,
        external_reply: Optional[str] = None,
        external_audience: str = "All",
    ) -> Dict[str, Any]:
        """Enable OOF immediately. Mirrors ``rules_oof.enable_oof``."""
        raise NotImplementedError

    def schedule_oof(
        self,
        start: str,
        end: str,
        internal_reply: str,
        external_reply: Optional[str] = None,
        external_audience: str = "All",
    ) -> Dict[str, Any]:
        """Schedule OOF for a future period. Mirrors ``rules_oof.schedule_oof``."""
        raise NotImplementedError

    def disable_oof(self) -> Dict[str, Any]:
        """Disable OOF. Mirrors ``rules_oof.disable_oof``."""
        raise NotImplementedError


class Backend(
    MailBackend,
    CalendarBackend,
    ContactsBackend,
    AvailabilityBackend,
    DirectoryBackend,
    SettingsBackend,
    ABC,
):
    """A provider backend for a single account.

    Concrete subclasses set :attr:`account`, :attr:`provider` and
    :attr:`capabilities` (typically in ``__init__``) and override the domain
    methods they support. :meth:`refresh` is the only abstract member.

    Attributes:
        account: The :class:`AccountConfig` this backend serves.
        provider: Provider id (mirrors ``account.provider``).
        capabilities: The frozenset of ``CAP_*`` values this backend supports.
    """

    account: AccountConfig
    provider: str
    capabilities: frozenset

    @abstractmethod
    def refresh(self) -> None:
        """Drop cached credentials/session and re-authenticate on next use."""
        raise NotImplementedError

    def supports(self, capability: str) -> bool:
        """Return whether ``capability`` is advertised by this backend."""
        return capability in self.capabilities
