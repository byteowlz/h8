"""Unit tests for :mod:`h8.security` (key model, scope grammar, key store)."""

from __future__ import annotations

import os
import stat

import pytest

from h8 import security
from h8.security import (
    KeyStore,
    account_allowed,
    generate_token,
    hash_token,
    is_valid_scope,
    scope_matches,
    validate_scopes,
)


# --- Scope grammar --------------------------------------------------------


@pytest.mark.parametrize(
    "granted,required,expected",
    [
        (["mail:read"], "mail:read", True),
        (["mail:read"], "mail:write", False),
        (["mail:*"], "mail:send", True),
        (["mail:*"], "calendar:read", False),
        (["*:read"], "calendar:read", True),
        (["*:read"], "calendar:write", False),
        (["*:*"], "mail:send", True),
        (["*:*"], "keys:write", True),
        (["calendar:read", "mail:send"], "mail:send", True),
        ([], "mail:read", False),  # deny by default
        (["mail:read"], "admin:write", False),
    ],
)
def test_scope_matches(granted, required, expected):
    assert scope_matches(granted, required) is expected


def test_is_valid_scope():
    assert is_valid_scope("mail:read")
    assert is_valid_scope("*:*")
    assert is_valid_scope("calendar:*")
    assert not is_valid_scope("bogus:read")
    assert not is_valid_scope("mail:destroy")
    assert not is_valid_scope("mail")
    assert not is_valid_scope("mail:read:extra")


def test_validate_scopes_raises_on_unknown_resource():
    with pytest.raises(ValueError, match="unknown resource 'bogus'"):
        validate_scopes(["mail:read", "bogus:read"])


def test_validate_scopes_raises_on_unknown_action():
    with pytest.raises(ValueError, match="unknown action 'destroy'"):
        validate_scopes(["mail:destroy"])


def test_validate_scopes_accepts_wildcards():
    validate_scopes(["*:*", "mail:*", "*:read"])  # no exception


# --- Token hashing --------------------------------------------------------


def test_generate_token_format():
    token = generate_token()
    assert token.startswith("h8k_")
    # 32 random bytes -> 43 urlsafe base64 chars.
    assert len(token) == len("h8k_") + 43


def test_hash_token_is_sha256_hex():
    digest = hash_token("h8k_abc")
    assert len(digest) == 64
    assert digest == hash_token("h8k_abc")
    assert digest != hash_token("h8k_abd")


# --- Account restriction --------------------------------------------------


def test_account_allowed_no_restriction():
    key = {"accounts": None}
    assert account_allowed(key, "work")
    assert account_allowed(key, None)


def test_account_allowed_with_restriction():
    key = {"accounts": ["work"]}
    assert account_allowed(key, "work")
    assert not account_allowed(key, "personal")
    # A missing account param falls back to the default account -> allowed.
    assert account_allowed(key, None)


# --- KeyStore -------------------------------------------------------------


@pytest.fixture
def store(tmp_path):
    return KeyStore(path=tmp_path / "keys.json")


def test_create_and_verify(store):
    record, token = store.create_key("reader", ["mail:read"])
    assert token.startswith("h8k_")
    assert record["name"] == "reader"
    assert record["scopes"] == ["mail:read"]
    assert "hash" in record
    matched = store.verify_token(token)
    assert matched is not None
    assert matched["id"] == record["id"]


def test_verify_unknown_token_returns_none(store):
    store.create_key("reader", ["mail:read"])
    assert store.verify_token("h8k_nope") is None
    assert store.verify_token("") is None


def test_disabled_key_does_not_verify(store):
    record, token = store.create_key("reader", ["mail:read"])
    assert store.verify_token(token) is not None
    assert store.revoke(record["id"]) is True
    assert store.verify_token(token) is None


def test_revoke_unknown_returns_false(store):
    assert store.revoke("k_missing") is False


def test_has_enabled_key(store):
    assert store.has_enabled_key() is False
    record, _ = store.create_key("reader", ["mail:read"])
    assert store.has_enabled_key() is True
    store.revoke(record["id"])
    assert store.has_enabled_key() is False


def test_create_rejects_invalid_scope(store):
    with pytest.raises(ValueError):
        store.create_key("bad", ["bogus:read"])


def test_persistence_across_instances(store, tmp_path):
    record, token = store.create_key("reader", ["mail:read"], accounts=["work"])
    other = KeyStore(path=tmp_path / "keys.json")
    matched = other.verify_token(token)
    assert matched is not None
    assert matched["accounts"] == ["work"]


def test_keys_file_permissions_are_0600(store):
    store.create_key("reader", ["mail:read"])
    mode = stat.S_IMODE(os.stat(store.path).st_mode)
    assert mode == 0o600


def test_last_used_at_is_throttled(store, monkeypatch):
    _record, token = store.create_key("reader", ["mail:read"])
    writes = {"count": 0}
    real_save = store.save

    def _counting_save():
        writes["count"] += 1
        real_save()

    monkeypatch.setattr(store, "save", _counting_save)
    # First verify sets last_used_at (one write); a rapid second verify is
    # throttled and must not write again.
    store.verify_token(token)
    store.verify_token(token)
    assert writes["count"] == 1


def test_public_key_view_hides_hash(store):
    record, _ = store.create_key("reader", ["mail:read"])
    view = security.public_key_view(record)
    assert "hash" not in view
    assert view["id"] == record["id"]
    assert view["scopes"] == ["mail:read"]
    assert view["disabled"] is False
