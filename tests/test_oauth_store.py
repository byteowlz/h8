"""Tests for the OAuth TokenStore file backend.

These exercise the 0600 JSON-file fallback only -- no keyring backend and no
network. ``use_keyring=False`` forces the file backend regardless of the host's
keyring configuration.
"""

import json
import stat

import pytest

from h8.oauth.store import TokenStore, _token_file_path


@pytest.fixture
def store(tmp_path, monkeypatch):
    """A file-backed TokenStore rooted at a temporary XDG_STATE_HOME."""
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
    return TokenStore(use_keyring=False)


class TestTokenFilePath:
    """Tests for the fallback path resolution."""

    def test_uses_xdg_state_home(self, tmp_path, monkeypatch):
        monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
        assert _token_file_path() == tmp_path / "h8" / "tokens.json"

    def test_defaults_to_local_state(self, monkeypatch):
        monkeypatch.delenv("XDG_STATE_HOME", raising=False)
        path = _token_file_path()
        assert path.parts[-4:] == (".local", "state", "h8", "tokens.json")


class TestFileBackend:
    """Tests for get/set/delete against the JSON file backend."""

    def test_get_missing_returns_none(self, store):
        assert store.get("ms:nobody@example.com") is None

    def test_roundtrip(self, store):
        store.set("ms:user@example.com", "serialized-cache-blob")
        assert store.get("ms:user@example.com") == "serialized-cache-blob"

    def test_overwrite(self, store):
        store.set("google:u@example.com", "first")
        store.set("google:u@example.com", "second")
        assert store.get("google:u@example.com") == "second"

    def test_multiple_keys_independent(self, store):
        store.set("a", "1")
        store.set("b", "2")
        assert store.get("a") == "1"
        assert store.get("b") == "2"

    def test_delete_removes_key(self, store):
        store.set("k", "v")
        store.delete("k")
        assert store.get("k") is None

    def test_delete_missing_is_noop(self, store):
        # Should not raise.
        store.delete("never-set")
        assert store.get("never-set") is None

    def test_opaque_string_values_preserved(self, store):
        blob = json.dumps({"nested": {"token": "abc"}, "n": 1})
        store.set("k", blob)
        assert store.get("k") == blob

    def test_file_created_at_expected_path(self, store, tmp_path):
        store.set("k", "v")
        assert (tmp_path / "h8" / "tokens.json").is_file()

    def test_file_has_0600_permissions(self, store, tmp_path):
        store.set("k", "v")
        path = tmp_path / "h8" / "tokens.json"
        mode = stat.S_IMODE(path.stat().st_mode)
        assert mode == 0o600

    def test_permissions_preserved_after_update(self, store, tmp_path):
        store.set("k", "v")
        store.set("k2", "v2")
        path = tmp_path / "h8" / "tokens.json"
        assert stat.S_IMODE(path.stat().st_mode) == 0o600

    def test_persists_across_instances(self, tmp_path, monkeypatch):
        monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
        TokenStore(use_keyring=False).set("k", "v")
        assert TokenStore(use_keyring=False).get("k") == "v"

    def test_corrupt_file_treated_as_empty(self, store, tmp_path):
        path = tmp_path / "h8" / "tokens.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("not json{{")
        assert store.get("anything") is None
        # And a subsequent set recovers cleanly.
        store.set("k", "v")
        assert store.get("k") == "v"
