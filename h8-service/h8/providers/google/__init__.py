"""Google Workspace backend -- NOT YET IMPLEMENTED (Phase 2).

This is a Phase-1 stub. It is registered in the provider registry so that route
wiring and capability introspection have a real provider to resolve, but any
attempt to construct it raises :class:`BackendNotSupported`.

Phase-2 agents flesh this out: the gmail agent adds ``client.py`` (credential ->
service builders) and a real ``GoogleBackend`` class here; the gcal/gcontacts
agents add ``calendar.py`` / ``contacts.py`` / ``settings.py`` modules and wire
their mixins in. Do not edit the registry to add Google -- it is already
registered.
"""

from __future__ import annotations

from h8.providers.base import AccountConfig, BackendNotSupported


class GoogleBackend:
    """Placeholder for the Google Workspace backend (Phase 2)."""

    def __init__(self, account: AccountConfig) -> None:
        raise BackendNotSupported("google backend not yet implemented")
