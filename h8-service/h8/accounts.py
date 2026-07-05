"""Account resolution from the ``[accounts.*]`` config registry.

Resolution rules (see docs/design/multi-provider.md section 1):

- A reference may be an **alias** (``work``) or a bare **email**.
- Alias lookup: match an ``[accounts.<alias>]`` table.
- Email lookup: match ``accounts.*.email``; an unmatched email yields an implicit
  EWS account (legacy behaviour, so ``--account someone.else@corp.com`` for
  delegate access keeps working).
- No reference: use the configured default (top-level ``account =`` -- an alias
  or a bare email). The legacy top-level ``account = "email"`` therefore keeps
  working and implicitly defines an EWS account.
"""

from __future__ import annotations

from typing import Any, Dict, Optional

from h8.config import get_config
from h8.providers.base import (
    PROVIDER_EWS,
    AccountConfig,
)

# Provider-neutral placeholder: there is no hardcoded default mailbox anymore.
# A default account must be supplied via config (top-level ``account`` or an
# ``[accounts.*]`` table).
_KNOWN_ACCOUNT_KEYS = {"email", "provider", "client_id", "tenant"}


class AccountResolutionError(Exception):
    """Raised when a reference cannot be resolved to an account.

    Distinct from provider/backend errors so the service layer can map it to an
    HTTP 400 (bad request / misconfiguration) rather than a 500.
    """


def _account_tables() -> Dict[str, dict]:
    """Return the ``[accounts.*]`` tables from config (possibly empty)."""
    config = get_config()
    accounts = config.get("accounts")
    if isinstance(accounts, dict):
        return accounts
    return {}


def _config_from_table(alias: str, table: dict) -> AccountConfig:
    """Build an :class:`AccountConfig` from an ``[accounts.<alias>]`` table."""
    email = table.get("email")
    if not email:
        raise AccountResolutionError(
            f"Account '{alias}' is missing a required 'email' field in config"
        )
    extra: Dict[str, Any] = {
        k: v for k, v in table.items() if k not in _KNOWN_ACCOUNT_KEYS
    }
    return AccountConfig(
        email=email,
        provider=table.get("provider", PROVIDER_EWS),
        alias=alias,
        client_id=table.get("client_id"),
        tenant=table.get("tenant", "organizations"),
        extra=extra,
    )


def _implicit_ews_account(email: str) -> AccountConfig:
    """An account referenced by bare email that is not in the config registry.

    Preserves legacy behaviour: any email is usable as an EWS/M365 mailbox.
    """
    return AccountConfig(email=email, provider=PROVIDER_EWS, alias=None)


def resolve_account(ref: Optional[str] = None) -> AccountConfig:
    """Resolve ``ref`` to an :class:`AccountConfig`.

    Args:
        ref: An alias, a bare email, or ``None`` for the configured default.

    Returns:
        The resolved account configuration.

    Raises:
        AccountResolutionError: When ``ref`` is ``None`` and no default is
            configured.
    """
    if ref is None:
        default = get_config().get("account")
        if not default:
            raise AccountResolutionError(
                "No account specified and no default configured. Set a top-level "
                "'account = \"<alias-or-email>\"' or an [accounts.*] table in "
                "config.toml."
            )
        ref = default

    tables = _account_tables()

    # 1. Alias match.
    if ref in tables:
        return _config_from_table(ref, tables[ref])

    # 2. Email match against a configured account.
    for alias, table in tables.items():
        if isinstance(table, dict) and table.get("email") == ref:
            return _config_from_table(alias, table)

    # 3. Bare email with no config entry -> implicit EWS account.
    if "@" in ref:
        return _implicit_ews_account(ref)

    # 4. An alias that does not exist and is not an email.
    available = ", ".join(sorted(tables.keys())) if tables else "none configured"
    raise AccountResolutionError(
        f"Unknown account '{ref}'. Configure an [accounts.{ref}] table or pass an "
        f"email address. Available account aliases: {available}."
    )
