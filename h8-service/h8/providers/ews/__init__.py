"""EWS backend: a thin adapter over the existing ``h8/*.py`` EWS modules.

This backend delegates every operation to the module functions in ``h8/mail.py``,
``h8/calendar.py``, etc. Those modules are the EWS implementation detail and are
NOT moved or rewritten in this milestone. The exchangelib ``Account`` construction
(previously in ``h8/auth.py``) now lives here.

Token acquisition still goes through ``h8.auth.get_token`` (oama) for now; the
OAuth swap is a later phase. ``refresh()`` forces a fresh token + ``Account``.

exchangelib is imported at module scope on purpose: this module is only imported
by the registry when an EWS backend is actually constructed, so non-EWS callers
never pay the import cost.
"""

from __future__ import annotations

import logging
import threading
import time
from typing import Any, Callable, Optional

from exchangelib import (
    DELEGATE,
    Account,
    Configuration,
    OAuth2AuthorizationCodeCredentials,
)
from exchangelib.errors import ErrorServerBusy, UnauthorizedError

from h8 import (
    calendar as _calendar,
    contacts as _contacts,
    free as _free,
    mail as _mail,
    people as _people,
    resolve as _resolve,
    resources as _resources,
    rules_oof as _rules_oof,
    unsubscribe as _unsubscribe,
)
from h8.auth import get_token
from h8.providers.base import (
    ALL_CAPABILITIES,
    PROVIDER_EWS,
    AccountConfig,
    Backend,
    BackendAuthError,
    BackendBusyError,
    BackendError,
)

log = logging.getLogger(__name__)

EWS_SERVER = "outlook.office365.com"


class EwsBackend(Backend):
    """Exchange Web Services backend for a single M365/Exchange mailbox."""

    def __init__(self, account: AccountConfig) -> None:
        self.account = account
        self.provider = PROVIDER_EWS
        self.capabilities = ALL_CAPABILITIES
        self._ews_account: Optional[Account] = None
        self._created_at: float = 0.0
        self._lock = threading.Lock()

    # -- account lifecycle --------------------------------------------------

    def _build_account(self) -> Account:
        """Construct an authenticated exchangelib ``Account`` for this mailbox."""
        email = self.account.email
        log.info("Building EWS account for %s", email)
        token = get_token(email)
        credentials = OAuth2AuthorizationCodeCredentials(
            access_token={"access_token": token, "token_type": "Bearer"}
        )
        config = Configuration(server=EWS_SERVER, credentials=credentials)
        return Account(
            primary_smtp_address=email,
            config=config,
            autodiscover=False,
            access_type=DELEGATE,
        )

    @property
    def ews_account(self) -> Account:
        """The underlying exchangelib ``Account`` (built lazily, cached)."""
        with self._lock:
            if self._ews_account is None:
                self._ews_account = self._build_account()
                self._created_at = time.time()
            return self._ews_account

    def refresh(self) -> None:
        """Drop the cached account and re-authenticate with a fresh token."""
        with self._lock:
            log.info("Refreshing EWS account for %s", self.account.email)
            self._ews_account = self._build_account()
            self._created_at = time.time()

    @property
    def created_at(self) -> float:
        """Wall-clock time the current account was built (0.0 if not built)."""
        return self._created_at

    # -- call wrapper -------------------------------------------------------

    def _call(self, func: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
        """Invoke an EWS module function, translating exchangelib errors.

        The underlying account is supplied as the leading positional argument.
        """
        try:
            return func(self.ews_account, *args, **kwargs)
        except UnauthorizedError as exc:
            raise BackendAuthError(str(exc)) from exc
        except ErrorServerBusy as exc:
            retry_after = getattr(exc, "back_off", None)
            raise BackendBusyError(str(exc), retry_after=retry_after) from exc
        except BackendError:
            raise
        except Exception as exc:  # noqa: BLE001 - surfaced as a generic backend error
            raise BackendError(str(exc)) from exc

    # -- MailBackend --------------------------------------------------------

    def list_messages(self, folder="inbox", limit=20, unread=False):
        return self._call(_mail.list_messages, folder, limit, unread)

    def search_messages(self, query, folder="inbox", limit=50, from_date=None, to_date=None):
        return self._call(_mail.search_messages, query, folder, limit, from_date, to_date)

    def get_message(self, item_id, folder="inbox"):
        return self._call(_mail.get_message, item_id, folder)

    def batch_get_messages(self, item_ids, folder="inbox"):
        return self._call(_mail.batch_get_messages, item_ids, folder)

    def send_message(self, message_data):
        return self._call(_mail.send_message, message_data)

    def fetch_messages(self, folder, output_dir, format="maildir", limit=None):
        return self._call(_mail.fetch_messages, folder, output_dir, format, limit)

    def save_draft(self, draft_data):
        return self._call(_mail.save_draft, draft_data)

    def update_draft(self, item_id, update_data):
        return self._call(_mail.update_draft, item_id, update_data)

    def delete_draft(self, item_id):
        return self._call(_mail.delete_draft, item_id)

    def list_attachments(self, item_id, folder="inbox"):
        return self._call(_mail.list_attachments, item_id, folder)

    def download_attachment(self, item_id, attachment_index, output_path, folder="inbox"):
        return self._call(
            _mail.download_attachment, item_id, attachment_index, output_path, folder
        )

    def delete_message(self, item_id, folder="inbox", permanent=False):
        return self._call(_mail.delete_message, item_id, folder, permanent)

    def move_message(self, item_id, target_folder, source_folder="inbox", create_folder=False):
        return self._call(
            _mail.move_message, item_id, target_folder, source_folder, create_folder
        )

    def empty_folder(self, folder_name="trash"):
        return self._call(_mail.empty_folder, folder_name)

    def batch_move_messages(
        self,
        folder,
        target_folder,
        older_than_days,
        query=None,
        limit=500,
        create_folder=True,
        dry_run=False,
    ):
        return self._call(
            _mail.batch_move_messages,
            folder,
            target_folder,
            older_than_days,
            query,
            limit,
            create_folder,
            dry_run,
        )

    def batch_mark_messages(
        self,
        folder,
        read,
        ids=None,
        older_than_days=None,
        query=None,
        limit=500,
        dry_run=False,
    ):
        return self._call(
            _mail.batch_mark_messages,
            folder,
            read,
            ids,
            older_than_days,
            query,
            limit,
            dry_run,
        )

    def mark_as_spam(self, item_id, is_spam=True, move_to_junk=True):
        return self._call(_mail.mark_as_spam, item_id, is_spam, move_to_junk)

    def scan_unsubscribe(
        self,
        folder="inbox",
        sender=None,
        search=None,
        limit=50,
        safe_senders=None,
        blocked_patterns=None,
    ):
        return self._call(
            _unsubscribe.scan_messages,
            folder,
            sender,
            search,
            limit,
            safe_senders,
            blocked_patterns,
        )

    def execute_unsubscribe(
        self,
        item_ids,
        safe_senders=None,
        blocked_patterns=None,
        trusted_domains=None,
        rate_limit_seconds=2.0,
    ):
        return self._call(
            _unsubscribe.execute_unsubscribe,
            item_ids,
            safe_senders,
            blocked_patterns,
            trusted_domains,
            rate_limit_seconds,
        )

    # -- CalendarBackend ----------------------------------------------------

    def list_events(self, days=7, from_date=None, to_date=None):
        return self._call(_calendar.list_events, days, from_date, to_date)

    def get_event(self, item_id):
        return self._call(_calendar.get_event, item_id)

    def create_event(self, event_data):
        return self._call(_calendar.create_event, event_data)

    def invite_event(self, event_data):
        return self._call(_calendar.invite_event, event_data)

    def delete_event(self, item_id):
        return self._call(_calendar.delete_event, item_id)

    def cancel_event(self, item_id, message=None):
        return self._call(_calendar.cancel_event, item_id, message)

    def search_events(self, query, days=90, from_date=None, to_date=None, limit=50):
        return self._call(
            _calendar.search_events, query, days, from_date, to_date, limit
        )

    def list_invites(self, limit=50):
        return self._call(_calendar.list_invites, limit)

    def rsvp_event(self, item_id, response, message=None):
        return self._call(_calendar.rsvp_event, item_id, response, message)

    # -- ContactsBackend ----------------------------------------------------

    def list_contacts(self, limit=100, search=None):
        return self._call(_contacts.list_contacts, limit, search)

    def get_contact(self, item_id):
        return self._call(_contacts.get_contact, item_id)

    def create_contact(self, contact_data):
        return self._call(_contacts.create_contact, contact_data)

    def delete_contact(self, item_id):
        return self._call(_contacts.delete_contact, item_id)

    def update_contact(self, item_id, updates):
        return self._call(_contacts.update_contact, item_id, updates)

    # -- AvailabilityBackend ------------------------------------------------

    def find_free_slots(
        self,
        weeks=1,
        duration_minutes=30,
        limit=None,
        start_hour=None,
        end_hour=None,
        exclude_weekends=None,
    ):
        return self._call(
            _free.find_free_slots,
            weeks,
            duration_minutes,
            limit,
            start_hour,
            end_hour,
            exclude_weekends,
        )

    def get_person_agenda(self, email, days=7, from_date=None, to_date=None):
        return self._call(_people.get_person_agenda, email, days, from_date, to_date)

    def get_person_free_slots(
        self,
        email,
        weeks=1,
        duration_minutes=30,
        limit=None,
        start_hour=None,
        end_hour=None,
        exclude_weekends=None,
    ):
        return self._call(
            _people.get_person_free_slots,
            email,
            weeks,
            duration_minutes,
            limit,
            start_hour,
            end_hour,
            exclude_weekends,
        )

    def find_common_free_slots(
        self,
        emails,
        weeks=1,
        duration_minutes=30,
        limit=None,
        start_hour=None,
        end_hour=None,
        exclude_weekends=None,
    ):
        return self._call(
            _people.find_common_free_slots,
            emails,
            weeks,
            duration_minutes,
            limit,
            start_hour,
            end_hour,
            exclude_weekends,
        )

    def resource_free(
        self, resources, from_date=None, to_date=None, days=1, start_hour=None, end_hour=None
    ):
        return self._call(
            _resources.resource_free,
            resources,
            from_date,
            to_date,
            days,
            start_hour,
            end_hour,
        )

    def resource_free_window(self, resources, from_date, to_date):
        return self._call(
            _resources.resource_free_window, resources, from_date, to_date
        )

    def resource_agenda(self, resources, from_date=None, to_date=None, days=1):
        return self._call(
            _resources.resource_agenda, resources, from_date, to_date, days
        )

    # -- DirectoryBackend ---------------------------------------------------

    def resolve_names(self, query):
        return self._call(_resolve.resolve_names, query)

    def validate_email(self, email):
        return self._call(_resolve.validate_email, email)

    # -- SettingsBackend ----------------------------------------------------

    def list_rules(self):
        return self._call(_rules_oof.list_rules)

    def get_rule(self, rule_id):
        return self._call(_rules_oof.get_rule, rule_id)

    def create_rule(
        self, display_name, priority=1, is_enabled=True, conditions=None, actions=None
    ):
        return self._call(
            _rules_oof.create_rule,
            display_name,
            priority,
            is_enabled,
            conditions,
            actions,
        )

    def update_rule(
        self,
        rule_id,
        display_name=None,
        priority=None,
        is_enabled=None,
        conditions=None,
        actions=None,
    ):
        return self._call(
            _rules_oof.update_rule,
            rule_id,
            display_name,
            priority,
            is_enabled,
            conditions,
            actions,
        )

    def enable_rule(self, rule_id):
        return self._call(_rules_oof.enable_rule, rule_id)

    def disable_rule(self, rule_id):
        return self._call(_rules_oof.disable_rule, rule_id)

    def delete_rule(self, rule_id):
        return self._call(_rules_oof.delete_rule, rule_id)

    def get_oof_settings(self):
        return self._call(_rules_oof.get_oof_settings)

    def set_oof_settings(
        self,
        state,
        external_audience=None,
        start=None,
        end=None,
        internal_reply=None,
        external_reply=None,
    ):
        return self._call(
            _rules_oof.set_oof_settings,
            state,
            external_audience,
            start,
            end,
            internal_reply,
            external_reply,
        )

    def enable_oof(self, internal_reply, external_reply=None, external_audience="All"):
        return self._call(
            _rules_oof.enable_oof, internal_reply, external_reply, external_audience
        )

    def schedule_oof(
        self, start, end, internal_reply, external_reply=None, external_audience="All"
    ):
        return self._call(
            _rules_oof.schedule_oof,
            start,
            end,
            internal_reply,
            external_reply,
            external_audience,
        )

    def disable_oof(self):
        return self._call(_rules_oof.disable_oof)
