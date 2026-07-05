"""Token storage: OS keyring with a 0600 file fallback.

``TokenStore`` persists opaque string values (serialized MSAL caches, Google
credential JSON, ...) keyed by short strings such as ``ms:user@example.com``.

Backend selection:
- The OS keyring (via the :mod:`keyring` package, service name ``h8``) is used
  when it is importable and a usable backend is present.
- On ``ImportError`` or any keyring backend failure -- at construction or during
  an operation -- the store falls back to a JSON file at
  ``$XDG_STATE_HOME/h8/tokens.json`` (default ``~/.local/state/h8/tokens.json``)
  created with mode 0600.
"""

import json
import logging
import os
import threading
from pathlib import Path
from typing import Optional

log = logging.getLogger(__name__)

SERVICE_NAME = "h8"


def _token_file_path() -> Path:
    """Return the fallback token file path, honouring ``XDG_STATE_HOME``."""
    base = os.environ.get("XDG_STATE_HOME")
    root = Path(base) if base else Path.home() / ".local" / "state"
    return root / "h8" / "tokens.json"


class _FileBackend:
    """JSON-file token backend with 0600 permissions."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._lock = threading.Lock()

    def _load(self) -> dict[str, str]:
        if not self.path.exists():
            return {}
        try:
            return json.loads(self.path.read_text())
        except (json.JSONDecodeError, OSError) as exc:
            log.warning("Could not read token file %s: %s", self.path, exc)
            return {}

    def _save(self, data: dict[str, str]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.chmod(self.path.parent, 0o700)
        except OSError:
            pass
        # Create with 0600 up-front; chmod afterwards guarantees the mode even
        # when the file already existed.
        fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as handle:
            json.dump(data, handle)
        os.chmod(self.path, 0o600)

    def get(self, key: str) -> Optional[str]:
        with self._lock:
            return self._load().get(key)

    def set(self, key: str, value: str) -> None:
        with self._lock:
            data = self._load()
            data[key] = value
            self._save(data)

    def delete(self, key: str) -> None:
        with self._lock:
            data = self._load()
            if key in data:
                del data[key]
                self._save(data)


class _KeyringBackend:
    """Thin wrapper over :mod:`keyring` for a single service name."""

    def __init__(self, service: str) -> None:
        self.service = service
        self._keyring = None
        self.available = False
        try:
            import keyring
            from keyring.backends import fail

            backend = keyring.get_keyring()
            if isinstance(backend, fail.Keyring):
                log.debug("No usable keyring backend; using file fallback")
                return
            self._keyring = keyring
            self.available = True
        except Exception as exc:  # noqa: BLE001 - any failure means "unavailable"
            log.debug("keyring unavailable (%s); using file fallback", exc)

    def get(self, key: str) -> Optional[str]:
        return self._keyring.get_password(self.service, key)

    def set(self, key: str, value: str) -> None:
        self._keyring.set_password(self.service, key, value)

    def delete(self, key: str) -> None:
        from keyring.errors import PasswordDeleteError

        try:
            self._keyring.delete_password(self.service, key)
        except PasswordDeleteError:
            # Absent key -> treat delete as a no-op.
            pass


class TokenStore:
    """Opaque string store backed by the OS keyring or a 0600 JSON file."""

    def __init__(self, *, use_keyring: bool = True) -> None:
        self._file = _FileBackend(_token_file_path())
        self._keyring: Optional[_KeyringBackend] = None
        if use_keyring:
            candidate = _KeyringBackend(SERVICE_NAME)
            if candidate.available:
                self._keyring = candidate

    def _fallback(self, exc: Exception) -> None:
        log.warning("Keyring operation failed (%s); falling back to file store", exc)
        self._keyring = None

    def get(self, key: str) -> Optional[str]:
        """Return the stored value for ``key`` or ``None`` if absent."""
        if self._keyring is not None:
            try:
                return self._keyring.get(key)
            except Exception as exc:  # noqa: BLE001
                self._fallback(exc)
        return self._file.get(key)

    def set(self, key: str, value: str) -> None:
        """Persist ``value`` (an opaque string) under ``key``."""
        if self._keyring is not None:
            try:
                self._keyring.set(key, value)
                return
            except Exception as exc:  # noqa: BLE001
                self._fallback(exc)
        self._file.set(key, value)

    def delete(self, key: str) -> None:
        """Remove ``key`` from the store; missing keys are ignored."""
        if self._keyring is not None:
            try:
                self._keyring.delete(key)
                return
            except Exception as exc:  # noqa: BLE001
                self._fallback(exc)
        self._file.delete(key)


_default_store: Optional[TokenStore] = None
_default_lock = threading.Lock()


def get_default_store() -> TokenStore:
    """Return a process-wide shared :class:`TokenStore` singleton."""
    global _default_store
    with _default_lock:
        if _default_store is None:
            _default_store = TokenStore()
        return _default_store
