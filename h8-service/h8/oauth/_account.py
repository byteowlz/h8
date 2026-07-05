"""Lightweight structural type for account configuration.

The real ``AccountConfig`` lives in ``h8.accounts`` (built concurrently by the
seam agent). To keep the OAuth subsystem decoupled during Phase 1, provider
modules type against this ``Protocol`` instead of importing that dataclass. Any
object exposing these attributes -- including the future ``AccountConfig`` -- is
accepted at runtime.
"""

from typing import Any, Optional, Protocol, runtime_checkable


@runtime_checkable
class AccountLike(Protocol):
    """Structural view of an account required by the OAuth layer."""

    alias: str
    email: str
    provider: str
    client_id: Optional[str]
    tenant: Optional[str]
    extra: dict[str, Any]
