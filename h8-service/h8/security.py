"""API key model, scope grammar, and key store for service authentication.

This module is the authoritative implementation of Epic D (service auth). It has
no FastAPI dependency so it can be used both by the HTTP layer and by the
``h8-service keys`` CLI (which must work without the server running).

Key model
---------
Keys live in ``$XDG_STATE_HOME/h8/keys.json`` (mode 0600) as::

    {"keys": [{"id": "k_x7ab", "name": "claude-mail-reader",
      "hash": "<sha256 hex of token>", "scopes": ["mail:read"],
      "accounts": null, "created_at": "...", "last_used_at": "...",
      "disabled": false}]}

Tokens are ``h8k_`` + ``secrets.token_urlsafe(32)`` and are shown exactly once at
creation. Only the SHA-256 hex digest is persisted; lookup is a constant-time
compare (:func:`hmac.compare_digest`).

Scope grammar
-------------
``<resource>:<action>`` where ``resource`` is one of :data:`RESOURCES`, ``action``
is one of :data:`ACTIONS`, and ``*`` is a wildcard on either side. Deny is the
default: a key only grants what its scopes list allows. An optional ``accounts``
list restricts which account aliases/emails a key may target.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import logging
import os
import secrets
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

log = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Scope grammar
# ---------------------------------------------------------------------------

#: Valid scope resources.
RESOURCES = frozenset(
    {
        "mail",
        "calendar",
        "contacts",
        "addr",
        "resources",
        "trip",
        "rules",
        "oof",
        "unsubscribe",
        "auth",
        "keys",
        "admin",
    }
)

#: Valid scope actions.
ACTIONS = frozenset({"read", "write", "send"})

#: Token prefix; the remainder is ``secrets.token_urlsafe(32)`` (43 chars).
TOKEN_PREFIX = "h8k_"

#: Throttle window for persisting ``last_used_at`` (avoid a disk write per request).
LAST_USED_THROTTLE_SECONDS = 60.0


def _now_iso() -> str:
    """Return the current UTC time as an ISO-8601 string."""
    return datetime.now(timezone.utc).isoformat()


def parse_scope(scope: str) -> tuple[str, str]:
    """Split ``<resource>:<action>`` into its two parts.

    Args:
        scope: A scope string such as ``"mail:read"`` or ``"*:*"``.

    Returns:
        A ``(resource, action)`` tuple (parts may be ``"*"``).

    Raises:
        ValueError: If the scope is not exactly ``resource:action``.
    """
    parts = scope.split(":")
    if len(parts) != 2 or not parts[0] or not parts[1]:
        raise ValueError(f"invalid scope '{scope}': expected '<resource>:<action>'")
    return parts[0], parts[1]


def is_valid_scope(scope: str) -> bool:
    """Return whether ``scope`` is grammatically valid (resource/action known or ``*``)."""
    try:
        resource, action = parse_scope(scope)
    except ValueError:
        return False
    if resource != "*" and resource not in RESOURCES:
        return False
    if action != "*" and action not in ACTIONS:
        return False
    return True


def validate_scopes(scopes: List[str]) -> None:
    """Validate every scope in ``scopes`` against the grammar.

    Raises:
        ValueError: Naming the first unknown resource/action encountered.
    """
    for scope in scopes:
        try:
            resource, action = parse_scope(scope)
        except ValueError as exc:
            raise ValueError(str(exc))
        if resource != "*" and resource not in RESOURCES:
            raise ValueError(
                f"unknown resource '{resource}' in scope '{scope}' "
                f"(valid: {', '.join(sorted(RESOURCES))})"
            )
        if action != "*" and action not in ACTIONS:
            raise ValueError(
                f"unknown action '{action}' in scope '{scope}' "
                f"(valid: {', '.join(sorted(ACTIONS))})"
            )


def _scope_component_matches(granted: str, required: str) -> bool:
    """Match one scope component, honouring the ``*`` wildcard on the granted side."""
    return granted == "*" or granted == required


def scope_matches(granted: List[str], required: str) -> bool:
    """Return whether any granted scope satisfies ``required``.

    Wildcards apply on either side of a granted scope: ``mail:*`` grants every
    mail action, ``*:read`` grants read on every resource, ``*:*`` grants all.
    The required scope itself is expected to be concrete (e.g. ``mail:read``),
    but wildcards in it are tolerated symmetrically.

    Args:
        granted: The key's scope list.
        required: The scope the route demands, e.g. ``"mail:send"``.

    Returns:
        ``True`` if access is granted, ``False`` otherwise (deny by default).
    """
    try:
        req_resource, req_action = parse_scope(required)
    except ValueError:
        return False
    for scope in granted:
        try:
            g_resource, g_action = parse_scope(scope)
        except ValueError:
            continue
        if _scope_component_matches(
            g_resource, req_resource
        ) and _scope_component_matches(g_action, req_action):
            return True
    return False


def hash_token(token: str) -> str:
    """Return the SHA-256 hex digest of a raw token."""
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def generate_token() -> str:
    """Generate a new raw API token (``h8k_`` + 43 urlsafe chars)."""
    return TOKEN_PREFIX + secrets.token_urlsafe(32)


def generate_key_id() -> str:
    """Generate a short random key id (``k_`` + 8 hex chars)."""
    return "k_" + secrets.token_hex(4)


def account_allowed(key: Dict[str, Any], account_ref: Optional[str]) -> bool:
    """Return whether ``key`` may target ``account_ref``.

    A ``None`` restriction (``accounts`` is ``null``) allows any account. When a
    restriction list is present, ``account_ref`` must match one of its entries by
    canonical identity: the requested reference and each restriction entry are
    resolved via :func:`h8.accounts.resolve_account`, and access is granted when
    the resolved emails are equal (case-insensitive) or the aliases match. A raw
    string match is honoured first, so restriction works even without config.

    A ``None`` ``account_ref`` (the caller did not specify one, so the default
    account is used) is always permitted -- restriction only blocks explicit
    foreign targets. If the requested reference cannot be resolved, access is
    denied (an explicit foreign/unresolvable target must not slip through).

    Args:
        key: A key record.
        account_ref: The requested account alias/email, or ``None``.

    Returns:
        ``True`` if the account is permitted for this key.
    """
    restriction = key.get("accounts")
    if restriction is None:
        return True
    if account_ref is None:
        return True
    # Fast path: exact string match (alias or email as written).
    if account_ref in restriction:
        return True

    # Canonical identity: resolve alias<->email so a key restricted to ["work"]
    # accepts the account's email (and vice versa).
    from h8.accounts import AccountResolutionError, resolve_account

    try:
        requested = resolve_account(account_ref)
    except AccountResolutionError:
        # Explicit foreign / unresolvable target -> deny.
        return False

    requested_email = (requested.email or "").lower()
    for entry in restriction:
        try:
            allowed = resolve_account(entry)
        except AccountResolutionError:
            continue
        if requested_email and (allowed.email or "").lower() == requested_email:
            return True
        if requested.alias and allowed.alias == requested.alias:
            return True
    return False


def _state_dir() -> Path:
    """Return ``$XDG_STATE_HOME/h8`` (default ``~/.local/state/h8``)."""
    base = os.environ.get("XDG_STATE_HOME")
    root = Path(base) if base else Path.home() / ".local" / "state"
    return root / "h8"


def keys_file_path() -> Path:
    """Return the path to ``keys.json`` under the h8 state directory."""
    return _state_dir() / "keys.json"


def client_key_path() -> Path:
    """Return the path to the bootstrap ``client.key`` file."""
    return _state_dir() / "client.key"


class KeyStore:
    """Load/save API keys and verify bearer tokens against them.

    The store is backed by a JSON file (0600). Instances cache the parsed key
    list in memory and reload lazily; mutating operations persist immediately.
    ``verify_token`` throttles ``last_used_at`` writes to at most once per key
    per :data:`LAST_USED_THROTTLE_SECONDS` to avoid a disk write per request.
    """

    def __init__(self, path: Optional[Path] = None) -> None:
        """Initialize the store.

        Args:
            path: Override the keys file location (defaults to
                ``$XDG_STATE_HOME/h8/keys.json``). Useful for tests.
        """
        self.path = path or keys_file_path()
        self._lock = threading.Lock()
        self._keys: List[Dict[str, Any]] = []
        self._loaded = False

    # -- persistence -------------------------------------------------------

    def load(self) -> List[Dict[str, Any]]:
        """Load and return the key records (reads the file on every call)."""
        with self._lock:
            self._keys = self._read()
            self._loaded = True
            return list(self._keys)

    def _read(self) -> List[Dict[str, Any]]:
        if not self.path.exists():
            return []
        try:
            data = json.loads(self.path.read_text())
        except (json.JSONDecodeError, OSError) as exc:
            log.warning("Could not read keys file %s: %s", self.path, exc)
            return []
        keys = data.get("keys")
        return keys if isinstance(keys, list) else []

    def _ensure_loaded(self) -> None:
        if not self._loaded:
            self._keys = self._read()
            self._loaded = True

    def save(self) -> None:
        """Persist the current key list to disk with 0600 permissions."""
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.chmod(self.path.parent, 0o700)
        except OSError:
            pass
        fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as handle:
            json.dump({"keys": self._keys}, handle, indent=2)
        try:
            os.chmod(self.path, 0o600)
        except OSError:
            pass

    # -- queries -----------------------------------------------------------

    def list_keys(self) -> List[Dict[str, Any]]:
        """Return a copy of every key record (including disabled ones)."""
        with self._lock:
            self._ensure_loaded()
            return [dict(k) for k in self._keys]

    def has_enabled_key(self) -> bool:
        """Return whether at least one non-disabled key exists."""
        with self._lock:
            self._ensure_loaded()
            return any(not k.get("disabled", False) for k in self._keys)

    # -- mutations ---------------------------------------------------------

    def create_key(
        self,
        name: str,
        scopes: List[str],
        accounts: Optional[List[str]] = None,
    ) -> tuple[Dict[str, Any], str]:
        """Create a new key, persist it, and return ``(record, raw_token)``.

        Args:
            name: Human-readable key name.
            scopes: Scope strings (validated against the grammar).
            accounts: Optional account restriction list, or ``None`` for any.

        Returns:
            A ``(record, raw_token)`` pair. The raw token is only available here.

        Raises:
            ValueError: If any scope is invalid.
        """
        validate_scopes(scopes)
        token = generate_token()
        now = _now_iso()
        record = {
            "id": generate_key_id(),
            "name": name,
            "hash": hash_token(token),
            "scopes": list(scopes),
            "accounts": list(accounts) if accounts is not None else None,
            "created_at": now,
            "last_used_at": None,
            "disabled": False,
        }
        with self._lock:
            self._ensure_loaded()
            self._keys.append(record)
            self.save()
        return dict(record), token

    def revoke(self, key_id: str) -> bool:
        """Disable the key with ``key_id``. Returns whether a key was changed."""
        with self._lock:
            self._ensure_loaded()
            for key in self._keys:
                if key.get("id") == key_id and not key.get("disabled", False):
                    key["disabled"] = True
                    self.save()
                    return True
            return False

    def verify_token(self, token: str) -> Optional[Dict[str, Any]]:
        """Return the key record matching ``token`` or ``None``.

        Uses a constant-time compare against every enabled key's stored hash so
        the lookup does not leak timing about which key (if any) matched.
        Updates ``last_used_at`` at most once per throttle window.
        """
        if not token:
            return None
        candidate = hash_token(token)
        with self._lock:
            self._ensure_loaded()
            match: Optional[Dict[str, Any]] = None
            for key in self._keys:
                if key.get("disabled", False):
                    continue
                stored = key.get("hash", "")
                # Constant-time compare; run for every key to avoid early exit.
                if hmac.compare_digest(candidate, stored):
                    match = key
            if match is None:
                return None
            self._touch_last_used(match)
            return dict(match)

    def _touch_last_used(self, key: Dict[str, Any]) -> None:
        """Update ``last_used_at`` for ``key`` if the throttle window elapsed.

        Caller must hold ``self._lock``.
        """
        now = datetime.now(timezone.utc)
        last = key.get("last_used_at")
        if last is not None:
            try:
                prev = datetime.fromisoformat(last)
                if (now - prev).total_seconds() < LAST_USED_THROTTLE_SECONDS:
                    return
            except (ValueError, TypeError):
                pass
        key["last_used_at"] = now.isoformat()
        try:
            self.save()
        except OSError as exc:
            log.warning("Could not persist last_used_at: %s", exc)


def public_key_view(key: Dict[str, Any]) -> Dict[str, Any]:
    """Return a key record with the ``hash`` field stripped, for API responses."""
    return {
        "id": key.get("id"),
        "name": key.get("name"),
        "scopes": key.get("scopes", []),
        "accounts": key.get("accounts"),
        "created_at": key.get("created_at"),
        "last_used_at": key.get("last_used_at"),
        "disabled": key.get("disabled", False),
    }
