"""Resolve email addresses via EWS ResolveNames (Global Address List lookup).

Uses the EWS ResolveNames service to validate email addresses and search
the Global Address List (GAL) for mailboxes, including resource rooms,
equipment, and distribution lists.
"""

import logging
from typing import Any

from exchangelib.account import Account
from exchangelib.services import ResolveNames

log = logging.getLogger(__name__)


def resolve_names(account: Account, query: str) -> list[dict]:
    """Resolve a name or email query against the Global Address List.

    Uses EWS ResolveNames to search the GAL. This finds mailboxes that
    are not in the user's contacts, including resource rooms, equipment,
    and distribution lists.

    Args:
        account: The authenticated EWS account
        query: Search string (partial name, email prefix, etc.)

    Returns:
        List of resolved mailbox dictionaries with name, email, mailbox_type, etc.
    """
    svc = ResolveNames(protocol=account.protocol)
    # exchangelib quirk: _version_hint must be set manually
    svc._version_hint = account.version

    try:
        results = list(svc.call(unresolved_entries=[query]))
    except Exception as exc:
        log.warning("ResolveNames failed for query '%s': %s", query, exc)
        return []

    resolved = []
    for item in results:
        entry = _mailbox_to_dict(item)
        if entry:
            resolved.append(entry)

    return resolved


def validate_email(account: Account, email: str) -> bool:
    """Check if an email address resolves to a valid mailbox in EWS.

    Args:
        account: The authenticated EWS account
        email: Email address to validate

    Returns:
        True if the email resolves to a valid mailbox, False otherwise
    """
    results = resolve_names(account, email)
    if not results:
        return False

    # Check if any result exactly matches the queried email
    email_lower = email.lower()
    for entry in results:
        if entry.get("email", "").lower() == email_lower:
            return True

    # If we got results but none match exactly, it might be an ambiguous match.
    # For validation purposes, any result means the server knows about it.
    # But we should be strict: only exact matches count.
    return False


def _mailbox_to_dict(item: Any) -> dict | None:
    """Convert a ResolveNames result to a dictionary.

    ResolveNames can return Mailbox objects or Contact objects depending
    on the match type.
    """
    # Handle Mailbox objects (most common from GAL)
    if hasattr(item, "email_address") and item.email_address:
        return {
            "name": getattr(item, "name", None) or "",
            "email": item.email_address,
            "routing_type": getattr(item, "routing_type", "SMTP") or "SMTP",
            "mailbox_type": getattr(item, "mailbox_type", "Unknown") or "Unknown",
        }

    # Handle Contact objects (when the result is a full contact)
    if hasattr(item, "email_addresses"):
        email = None
        if item.email_addresses:
            for addr in item.email_addresses:
                if hasattr(addr, "email"):
                    email = addr.email
                    break
                elif isinstance(addr, str):
                    email = addr
                    break
        if email:
            name = getattr(item, "display_name", None) or getattr(item, "name", None) or ""
            return {
                "name": name,
                "email": email,
                "routing_type": "SMTP",
                "mailbox_type": "Contact",
            }

    log.debug("Skipping unrecognized ResolveNames result type: %s", type(item))
    return None
