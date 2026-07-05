"""Provider abstraction layer for h8 backends.

Public surface lives in :mod:`h8.providers.base` (contracts, capabilities,
exceptions) and :mod:`h8.providers.registry` (``get_backend``). Provider
implementation packages (``ews``, ``google``) are imported lazily by the
registry so that importing this package does not pull in any provider SDK.
"""
