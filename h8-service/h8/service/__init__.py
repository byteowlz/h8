"""
Background service that exposes the h8 provider backends over HTTP for the CLI.

Routes resolve an account reference to a :class:`~h8.providers.base.Backend` via
``h8.providers.registry.get_backend`` and invoke domain methods on it. Backend
construction and token acquisition happen inside the threadpool wrappers so the
event loop is never blocked.

Features:
- Per-provider backends (EWS today; Google/Graph later) behind one seam
- Capability introspection (``GET /capabilities``) and HTTP 501 for unsupported
  operations
- Automatic refresh+retry on ``BackendAuthError`` and backoff on ``BackendBusyError``
- Draft management endpoints
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import sys
import time
from contextlib import asynccontextmanager
from datetime import datetime, timezone
from functools import partial
from pathlib import Path
from typing import Any, Dict, List, Optional

import uvicorn
from fastapi import Depends, FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.concurrency import run_in_threadpool
from fastapi.dependencies.utils import get_parameterless_sub_dependant
from fastapi.responses import JSONResponse, Response
from fastapi.routing import APIRoute
from pydantic import BaseModel, Field, field_validator

from h8 import security
from h8.security import (
    KeyStore,
    account_allowed,
    client_key_path,
    public_key_view,
    scope_matches,
)
from h8.auth import AuthLoginRequired
from h8.accounts import AccountResolutionError, resolve_account
from h8.config import resolve_person_alias
from h8.providers.base import (
    CAP_CALENDAR,
    CAP_CALENDAR_MEETINGS,
    CAP_CONTACTS,
    CAP_FREEBUSY_OTHERS,
    CAP_FREEBUSY_SELF,
    CAP_GAL,
    CAP_MAIL,
    CAP_MAIL_FOLDERS,
    CAP_MAIL_SEND,
    CAP_OOF,
    CAP_RESOURCES,
    CAP_RULES,
    BackendAuthError,
    BackendBusyError,
    BackendError,
    BackendNotSupported,
)
from h8.providers.registry import get_backend, get_cache_info

log = logging.getLogger(__name__)

DEFAULT_PORT = int(os.environ.get("H8_SERVICE_PORT", "8787"))
DEFAULT_HOST = os.environ.get("H8_SERVICE_HOST", "127.0.0.1")
REFRESH_INTERVAL = int(os.environ.get("H8_SERVICE_REFRESH_SECONDS", "300"))
CACHE_TTL = int(os.environ.get("H8_SERVICE_CACHE_TTL", "300"))
LOG_LEVEL = os.environ.get("H8_SERVICE_LOGLEVEL", "INFO").upper()
# Token refresh interval in seconds (default: 45 minutes)
# OAuth tokens typically expire after 60 minutes, so refresh at 45 to be safe
TOKEN_REFRESH_INTERVAL = int(os.environ.get("H8_SERVICE_TOKEN_REFRESH_SECONDS", "2700"))


def _no_auth() -> bool:
    """Return whether the auth escape hatch (``H8_SERVICE_NO_AUTH``) is enabled."""
    return os.environ.get("H8_SERVICE_NO_AUTH", "") not in ("", "0", "false", "False")


def _audit_enabled() -> bool:
    """Return whether request auditing is enabled (``H8_SERVICE_AUDIT != 0``)."""
    return os.environ.get("H8_SERVICE_AUDIT", "") not in ("0", "false", "False")


#: Process-wide key store (lazy; no disk access at import time).
key_store = KeyStore()


class CacheEntry(BaseModel):
    data: Any
    ts: float

    def fresh(self) -> bool:
        return (time.time() - self.ts) < CACHE_TTL


class CalendarCreate(BaseModel):
    subject: str
    start: str
    end: str
    location: Optional[str] = None
    body: Optional[str] = None


class CalendarInvite(BaseModel):
    """Request model for sending meeting invites."""

    subject: str
    start: str
    end: str
    location: Optional[str] = None
    body: Optional[str] = None
    attendees: List[str] = Field(default_factory=list)
    required_attendees: List[str] = Field(default_factory=list)
    optional_attendees: List[str] = Field(default_factory=list)

    @field_validator(
        "attendees", "required_attendees", "optional_attendees", mode="before"
    )
    @classmethod
    def _coerce_list(cls, v):
        if v is None:
            return []
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


class CalendarRsvp(BaseModel):
    """Request model for responding to meeting invites."""

    response: str  # accept, decline, tentative
    message: Optional[str] = None


class SendEmail(BaseModel):
    to: list[str]
    cc: list[str] = Field(default_factory=list)
    subject: str
    body: str = ""
    html: bool = False
    schedule_at: Optional[str] = None  # ISO datetime for delayed delivery

    @field_validator("to", "cc", mode="before")
    @classmethod
    def _coerce_list(cls, v):
        if v is None:
            return []
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


class ContactCreate(BaseModel):
    display_name: Optional[str] = None
    name: Optional[str] = None
    given_name: Optional[str] = None
    surname: Optional[str] = None
    email: Optional[str] = None
    phone: Optional[str] = None
    company: Optional[str] = None
    job_title: Optional[str] = None


class ContactUpdate(BaseModel):
    """Request model for updating a contact."""

    display_name: Optional[str] = None
    given_name: Optional[str] = None
    surname: Optional[str] = None
    email: Optional[str] = None
    phone: Optional[str] = None
    company: Optional[str] = None
    job_title: Optional[str] = None


class FetchMail(BaseModel):
    folder: str = "inbox"
    output: str
    format: str = "maildir"
    limit: Optional[int] = None


class DraftSave(BaseModel):
    """Request model for saving a draft."""

    to: List[str] = Field(default_factory=list)
    cc: List[str] = Field(default_factory=list)
    bcc: List[str] = Field(default_factory=list)
    subject: str = ""
    body: str = ""
    html: bool = False
    in_reply_to: Optional[str] = None
    references: Optional[str] = None

    @field_validator("to", "cc", "bcc", mode="before")
    @classmethod
    def _coerce_list(cls, v):
        if v is None:
            return []
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


class DraftUpdate(BaseModel):
    """Request model for updating a draft."""

    to: Optional[List[str]] = None
    cc: Optional[List[str]] = None
    bcc: Optional[List[str]] = None
    subject: Optional[str] = None
    body: Optional[str] = None
    html: Optional[bool] = None

    @field_validator("to", "cc", "bcc", mode="before")
    @classmethod
    def _coerce_list(cls, v):
        if v is None:
            return None
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


# === Rules and OOF Models ===


class RuleCreate(BaseModel):
    """Request model for creating an inbox rule."""

    display_name: str
    priority: int = 1
    is_enabled: bool = True
    conditions: Optional[Dict[str, Any]] = None
    actions: Optional[Dict[str, Any]] = None


class RuleUpdate(BaseModel):
    """Request model for updating an inbox rule."""

    display_name: Optional[str] = None
    priority: Optional[int] = None
    is_enabled: Optional[bool] = None
    conditions: Optional[Dict[str, Any]] = None
    actions: Optional[Dict[str, Any]] = None


class OofSettings(BaseModel):
    """Request model for setting OOF."""

    state: str  # Enabled, Scheduled, or Disabled
    external_audience: Optional[str] = "All"  # All, Known, or None
    start: Optional[str] = None  # ISO datetime
    end: Optional[str] = None  # ISO datetime
    internal_reply: Optional[str] = None
    external_reply: Optional[str] = None


class OofEnable(BaseModel):
    """Request model for enabling OOF."""

    internal_reply: str
    external_reply: Optional[str] = None
    external_audience: str = "All"  # All, Known, or None


class OofSchedule(BaseModel):
    """Request model for scheduling OOF."""

    start: str  # ISO datetime
    end: str  # ISO datetime
    internal_reply: str
    external_reply: Optional[str] = None
    external_audience: str = "All"  # All, Known, or None


def current_account_email(requested: Optional[str]) -> str:
    """Resolve the account reference to an email, raising HTTP 400 if none."""
    try:
        return resolve_account(requested).email
    except AccountResolutionError as exc:
        raise HTTPException(status_code=400, detail=str(exc))


def _invoke_backend(
    account_ref: Optional[str],
    capability: Optional[str],
    method_name: str,
    do_refresh: bool,
    args: tuple,
    kwargs: dict,
) -> Any:
    """Resolve the backend and invoke ``method_name`` (runs in the threadpool).

    Constructs/looks up the backend (and thus acquires tokens) off the event
    loop, optionally refreshes it, enforces the capability, then delegates.
    """
    backend = get_backend(account_ref)
    if do_refresh:
        backend.refresh()
    if capability is not None and capability not in backend.capabilities:
        raise BackendNotSupported(
            f"not supported by provider '{backend.provider}' for account "
            f"'{backend.account.ref}'",
            capability=capability,
        )
    method = getattr(backend, method_name)
    return method(*args, **kwargs)


async def safe_call_with_retry(
    account_ref: Optional[str],
    capability: Optional[str],
    method_name: str,
    *args: Any,
    **kwargs: Any,
):
    """Invoke a backend method with capability gating, refresh-retry and backoff.

    - ``BackendAuthError``: call ``backend.refresh()`` and retry exactly once.
    - ``BackendBusyError``: back off (using ``retry_after`` if present) and retry.
    - ``BackendNotSupported``: HTTP 501 with ``missing_capability``.
    """
    max_retries = 3
    base_delay = 2  # seconds
    do_refresh = False

    for attempt in range(max_retries):
        try:
            return await run_in_threadpool(
                _invoke_backend,
                account_ref,
                capability,
                method_name,
                do_refresh,
                args,
                kwargs,
            )
        except AccountResolutionError as exc:
            raise HTTPException(status_code=400, detail=str(exc))
        except AuthLoginRequired as exc:
            # Interactive login required -- refresh+retry cannot help. Map
            # straight to 401 with the "run h8 auth login" guidance. Must be
            # caught before BackendAuthError (it is a subclass).
            log.warning("Login required for %s: %s", account_ref, exc)
            raise HTTPException(status_code=401, detail=str(exc))
        except BackendAuthError as exc:
            if do_refresh:
                log.error("Retry failed with BackendAuthError for %s", account_ref)
                raise HTTPException(
                    status_code=401, detail=f"Authentication failed: {exc}"
                )
            log.warning(
                "BackendAuthError for %s, refreshing token and retrying...",
                account_ref,
            )
            do_refresh = True
            continue
        except BackendBusyError as exc:
            if attempt < max_retries - 1:
                delay = (
                    exc.retry_after
                    if exc.retry_after is not None
                    else base_delay * (2**attempt)
                )
                log.warning(
                    "Provider busy for %s, retrying in %ss (attempt %d/%d)...",
                    account_ref,
                    delay,
                    attempt + 1,
                    max_retries,
                )
                await asyncio.sleep(delay)
                continue
            log.error("Provider busy retries exhausted for %s", account_ref)
            raise HTTPException(
                status_code=503,
                detail=f"Provider busy, please try again later: {exc}",
            )
        except BackendNotSupported as exc:
            return JSONResponse(
                status_code=501,
                content={"detail": str(exc), "missing_capability": exc.capability},
            )
        except HTTPException:
            raise
        except BackendError as exc:
            log.error("Backend error in %s: %s", method_name, exc)
            raise HTTPException(status_code=500, detail=str(exc))
        except Exception as exc:  # noqa: BLE001
            log.error("Error in %s: %s", method_name, exc)
            raise HTTPException(status_code=500, detail=str(exc))

    raise HTTPException(status_code=500, detail="retry loop exhausted")


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Startup/shutdown: configure logging, warm tokens, run background loops."""
    level = getattr(logging, LOG_LEVEL, logging.INFO)
    logging.basicConfig(level=level)
    # Bootstrap API-key auth before serving any request (mints a root key +
    # client.key on first run, or after a keys.json wipe).
    await run_in_threadpool(_bootstrap_auth)
    # Refresh tokens immediately on startup to ensure we have valid tokens.
    # (No credentials yet -> logged as a warning; run `h8 auth login <account>`.)
    await refresh_tokens()
    refresh_task = asyncio.create_task(_refresh_loop())
    token_refresh_task = asyncio.create_task(_token_refresh_loop())
    app.state.refresh_task = refresh_task
    app.state.token_refresh_task = token_refresh_task
    log.info("h8-service started on %s:%s", DEFAULT_HOST, DEFAULT_PORT)
    try:
        yield
    finally:
        for task in (refresh_task, token_refresh_task):
            task.cancel()
            with contextlib.suppress(Exception):
                await task
        log.info("h8-service shutdown")


app = FastAPI(title="h8-service", version="0.5.14", lifespan=lifespan)
cache: Dict[str, CacheEntry] = {}
cache_lock = asyncio.Lock()


# === Service authentication (Epic D / trx-x706) ===


class InsufficientScopeError(Exception):
    """Raised when an authenticated key lacks the scope a route requires.

    Mapped to HTTP 403 with a body carrying both ``detail`` and the exact
    ``required_scope`` string (the Rust client parses ``required_scope``).
    """

    def __init__(self, required_scope: str, detail: str) -> None:
        super().__init__(detail)
        self.required_scope = required_scope
        self.detail = detail


@app.exception_handler(InsufficientScopeError)
async def _insufficient_scope_handler(
    request: Request, exc: InsufficientScopeError
) -> JSONResponse:
    """Return the 403 body the Rust client expects (with ``required_scope``)."""
    return JSONResponse(
        status_code=403,
        content={"detail": exc.detail, "required_scope": exc.required_scope},
    )


def _enforce_scope(request: Request, required_scope: str) -> None:
    """Authenticate the request and check scope + account restriction.

    Raises:
        HTTPException: 401 for a missing/invalid token, 403 for an account the
            key may not target.
        InsufficientScopeError: 403 when the key lacks ``required_scope``.
    """
    if _no_auth():
        return
    header = request.headers.get("authorization", "")
    if not header.lower().startswith("bearer "):
        raise HTTPException(
            status_code=401,
            detail="Missing or malformed Authorization header; expected "
            "'Bearer <token>'",
        )
    token = header[7:].strip()
    key = key_store.verify_token(token)
    if key is None:
        raise HTTPException(status_code=401, detail="Invalid or revoked API token")
    if not scope_matches(key.get("scopes", []), required_scope):
        raise InsufficientScopeError(
            required_scope,
            f"API key '{key.get('name')}' lacks the required scope "
            f"'{required_scope}'",
        )
    account = request.query_params.get("account")
    if not account_allowed(key, account):
        raise HTTPException(
            status_code=403,
            detail=f"API key '{key.get('name')}' is restricted to accounts "
            f"{key.get('accounts')} and may not target account '{account}'",
        )
    # Stash minimal identity for the audit middleware.
    request.state.auth_key = {"id": key.get("id"), "name": key.get("name")}


def require_scope(scope: str):
    """Return a FastAPI dependency enforcing ``scope`` on a route."""

    async def _dependency(request: Request) -> None:
        _enforce_scope(request, scope)

    return _dependency


#: Route -> required scope. GET -> ``<resource>:read``; mutations ->
#: ``<resource>:write`` except mail send paths (``mail:send``). ``/health`` and
#: ``/capabilities`` are intentionally absent (unauthenticated).
ROUTE_SCOPES: Dict[tuple, str] = {
    # Calendar
    ("GET", "/calendar"): "calendar:read",
    ("GET", "/calendar/{item_id}"): "calendar:read",
    ("POST", "/calendar"): "calendar:write",
    ("POST", "/calendar/parse"): "calendar:read",
    ("DELETE", "/calendar/{item_id}"): "calendar:write",
    ("POST", "/calendar/{item_id}/cancel"): "calendar:write",
    ("GET", "/calendar/search"): "calendar:read",
    ("POST", "/calendar/invite"): "calendar:write",
    ("GET", "/calendar/invites"): "calendar:read",
    ("POST", "/calendar/{item_id}/rsvp"): "calendar:write",
    # Mail
    ("GET", "/mail"): "mail:read",
    ("GET", "/mail/search"): "mail:read",
    ("GET", "/mail/{item_id}"): "mail:read",
    ("POST", "/mail/batch"): "mail:read",
    ("POST", "/mail/send"): "mail:send",
    ("POST", "/mail/send-files"): "mail:send",
    ("POST", "/mail/fetch"): "mail:read",
    ("POST", "/mail/draft"): "mail:write",
    ("PUT", "/mail/draft/{item_id}"): "mail:write",
    ("DELETE", "/mail/draft/{item_id}"): "mail:write",
    ("GET", "/mail/{item_id}/attachments"): "mail:read",
    ("POST", "/mail/{item_id}/attachments/download"): "mail:read",
    ("DELETE", "/mail/{item_id}"): "mail:write",
    ("POST", "/mail/{item_id}/move"): "mail:write",
    ("DELETE", "/mail/folder/{folder_name}"): "mail:write",
    ("POST", "/mail/move-old"): "mail:write",
    ("POST", "/mail/mark"): "mail:write",
    ("POST", "/mail/{item_id}/spam"): "mail:write",
    ("POST", "/mail/unsubscribe/scan"): "unsubscribe:read",
    ("POST", "/mail/unsubscribe/execute"): "unsubscribe:write",
    # Contacts
    ("GET", "/contacts"): "contacts:read",
    ("GET", "/contacts/{item_id}"): "contacts:read",
    ("POST", "/contacts"): "contacts:write",
    ("DELETE", "/contacts/{item_id}"): "contacts:write",
    ("PUT", "/contacts/{item_id}"): "contacts:write",
    # Free / people (map to calendar:read)
    ("GET", "/free"): "calendar:read",
    ("GET", "/ppl/agenda"): "calendar:read",
    ("GET", "/ppl/free"): "calendar:read",
    ("POST", "/ppl/common"): "calendar:read",
    # Resources (reads; booking would be resources:write when added)
    ("POST", "/resource/free"): "resources:read",
    ("POST", "/resource/free-window"): "resources:read",
    ("POST", "/resource/agenda"): "resources:read",
    # Address / GAL
    ("GET", "/addr/resolve"): "addr:read",
    ("GET", "/addr/validate"): "addr:read",
    # Trip / routing
    ("POST", "/trip/geocode"): "trip:read",
    ("POST", "/trip/route"): "trip:read",
    # Rules
    ("GET", "/rules"): "rules:read",
    ("GET", "/rules/{rule_id}"): "rules:read",
    ("POST", "/rules"): "rules:write",
    ("PUT", "/rules/{rule_id}"): "rules:write",
    ("POST", "/rules/{rule_id}/enable"): "rules:write",
    ("POST", "/rules/{rule_id}/disable"): "rules:write",
    ("DELETE", "/rules/{rule_id}"): "rules:write",
    # Out-of-office
    ("GET", "/oof"): "oof:read",
    ("PUT", "/oof"): "oof:write",
    ("POST", "/oof/enable"): "oof:write",
    ("POST", "/oof/schedule"): "oof:write",
    ("POST", "/oof/disable"): "oof:write",
    # Auth administration
    ("GET", "/auth/accounts"): "admin:write",
    ("POST", "/auth/login"): "admin:write",
    ("GET", "/auth/login/{session_id}"): "admin:write",
    ("POST", "/auth/login/{session_id}/finish"): "admin:write",
    ("POST", "/auth/logout"): "admin:write",
    # Key management
    ("GET", "/keys"): "keys:read",
    ("POST", "/keys"): "keys:write",
    ("DELETE", "/keys/{key_id}"): "keys:write",
}


def _apply_route_scopes() -> None:
    """Attach the ``require_scope`` dependency to every route in ROUTE_SCOPES.

    Uses FastAPI's own parameterless sub-dependant mechanism (the same one
    router-level ``dependencies=`` uses) so the check runs after the route is
    matched and the ``account`` query param is available. Called once after all
    routes are registered.
    """
    covered: set = set()
    for route in app.routes:
        if not isinstance(route, APIRoute):
            continue
        for method in route.methods or ():
            scope = ROUTE_SCOPES.get((method, route.path))
            if scope is None:
                continue
            route.dependant.dependencies.append(
                get_parameterless_sub_dependant(
                    depends=Depends(require_scope(scope)), path=route.path
                )
            )
            covered.add((method, route.path))
    missing = set(ROUTE_SCOPES) - covered
    if missing:
        log.warning("ROUTE_SCOPES entries matched no route: %s", sorted(missing))


# === Host-header and audit middleware (hardening) ===


def _allowed_hosts() -> set:
    """Return the set of hostnames the Host header may carry."""
    hosts = {"localhost", "127.0.0.1", "::1", "[::1]"}
    configured = os.environ.get("H8_SERVICE_HOST")
    if configured:
        hosts.add(configured)
        hosts.add(f"[{configured}]")
    return hosts


def _host_hostname(host_header: str) -> str:
    """Extract the hostname (dropping any port) from a Host header value."""
    value = host_header.strip()
    if not value:
        return ""
    if value.startswith("["):
        # IPv6 literal, e.g. "[::1]" or "[::1]:8787".
        end = value.find("]")
        return value[: end + 1] if end != -1 else value
    if value.count(":") == 1:
        return value.rsplit(":", 1)[0]
    return value


@app.middleware("http")
async def audit_middleware(request: Request, call_next):
    """Append one JSONL audit line per authenticated request (except /health)."""
    start = time.monotonic()
    response = await call_next(request)
    try:
        if not _audit_enabled() or request.url.path == "/health":
            return response
        key = getattr(request.state, "auth_key", None)
        if key is None:
            return response
        duration_ms = round((time.monotonic() - start) * 1000, 2)
        _write_audit_line(
            {
                "ts": datetime.now(timezone.utc).isoformat(),
                "key_id": key.get("id"),
                "key_name": key.get("name"),
                "method": request.method,
                "path": request.url.path,
                "account": request.query_params.get("account"),
                "status": response.status_code,
                "duration_ms": duration_ms,
            }
        )
    except Exception:  # noqa: BLE001 -- auditing must never break a request
        log.exception("audit logging failed")
    return response


@app.middleware("http")
async def host_header_middleware(request: Request, call_next):
    """Reject requests whose Host header is not in the allowlist (DNS-rebinding).

    Runs before authentication (all middleware precedes route dependencies).
    """
    hostname = _host_hostname(request.headers.get("host", ""))
    if hostname and hostname not in _allowed_hosts():
        return JSONResponse(
            status_code=400,
            content={"detail": f"Host header '{hostname}' is not allowed"},
        )
    return await call_next(request)


def _audit_file_path() -> Path:
    """Return ``$XDG_STATE_HOME/h8/audit.jsonl``."""
    base = os.environ.get("XDG_STATE_HOME")
    root = Path(base) if base else Path.home() / ".local" / "state"
    return root / "h8" / "audit.jsonl"


def _write_audit_line(entry: dict) -> None:
    """Append one JSON object as a line to the audit log (open-append per write)."""
    path = _audit_file_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry) + "\n")


def _bootstrap_auth() -> None:
    """Ensure a usable root key + ``client.key`` exist (runs at startup).

    - No enabled key at all: mint a ``root`` key (``*:*``) and write its raw
      token to ``client.key`` (0600).
    - An enabled key exists but ``client.key`` matches none of them: mint a new
      root key so client and server stay in sync after a keys.json wipe.
    """
    if _no_auth():
        log.warning(
            "H8_SERVICE_NO_AUTH is set -- ALL API authentication is DISABLED. "
            "Do not use this outside local debugging."
        )
        return
    if not key_store.has_enabled_key():
        _mint_root_key("no enabled API keys present")
        return
    ckp = client_key_path()
    if ckp.exists():
        try:
            token = ckp.read_text().strip()
        except OSError:
            token = ""
        if key_store.verify_token(token) is None:
            _mint_root_key("client.key matched no enabled key")


def _mint_root_key(reason: str) -> None:
    """Create a root ``*:*`` key and persist its token to ``client.key`` (0600)."""
    _record, token = key_store.create_key("root", ["*:*"], None)
    path = client_key_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.chmod(path.parent, 0o700)
    except OSError:
        pass
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as handle:
        handle.write(token)
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass
    log.warning(
        "Bootstrapped root API key (%s); wrote client token to %s", reason, path
    )


def cache_key(prefix: str, **params: Any) -> str:
    parts = [prefix] + [f"{k}={v}" for k, v in sorted(params.items())]
    return "|".join(parts)


async def get_or_set(key: str, producer):
    """Get a cached value or produce and cache a new one.

    ``Response`` objects (e.g. a 501 for an unsupported capability) are returned
    but never cached.
    """
    async with cache_lock:
        entry = cache.get(key)
        if entry and entry.fresh():
            log.debug("cache hit: %s", key)
            return entry.data
    log.debug("cache miss: %s", key)
    data = await producer()
    if isinstance(data, Response):
        return data
    async with cache_lock:
        cache[key] = CacheEntry(data=data, ts=time.time())
    return data


def _refresh_default_backend() -> None:
    """Force-refresh the default account's backend (runs in the threadpool)."""
    backend = get_backend(None)
    backend.refresh()


async def refresh_defaults() -> None:
    """Refresh default cached queries for the configured default account."""
    try:
        await get_or_set(
            cache_key("calendar", account=None, days=7, from_date=None, to_date=None),
            partial(safe_call_with_retry, None, CAP_CALENDAR, "list_events", 7, None, None),
        )
        await get_or_set(
            cache_key("mail", account=None, folder="inbox", limit=20, unread=False),
            partial(
                safe_call_with_retry, None, CAP_MAIL, "list_messages", "inbox", 20, False
            ),
        )
        await get_or_set(
            cache_key("contacts", account=None, limit=100, search=None),
            partial(safe_call_with_retry, None, CAP_CONTACTS, "list_contacts", 100, None),
        )
        await get_or_set(
            cache_key("free", account=None, weeks=1, duration=30, limit=None),
            partial(
                safe_call_with_retry, None, CAP_FREEBUSY_SELF, "find_free_slots", 1, 30, None
            ),
        )
    except Exception:
        log.exception("default refresh failed")


async def refresh_tokens() -> None:
    """Proactively refresh the default account's token before it expires."""
    try:
        await run_in_threadpool(_refresh_default_backend)
        log.info("Proactively refreshed default account token")
    except Exception as e:
        log.warning("Token refresh failed: %s", e)


async def _refresh_loop():
    """Background loop to refresh cached data."""
    while True:
        await refresh_defaults()
        await asyncio.sleep(REFRESH_INTERVAL)


async def _token_refresh_loop():
    """Background loop to proactively refresh authentication tokens."""
    while True:
        await asyncio.sleep(TOKEN_REFRESH_INTERVAL)
        await refresh_tokens()


@app.get("/health")
async def health():
    """Health check endpoint."""
    cache_info = await run_in_threadpool(get_cache_info)
    return {
        "status": "ok",
        "accounts": cache_info,
    }


@app.get("/capabilities")
async def capabilities(account: Optional[str] = None):
    """Report the provider and capabilities for an account (unauthenticated)."""

    def _caps():
        acct = resolve_account(account)
        try:
            backend = get_backend(account)
            caps = sorted(backend.capabilities)
        except BackendNotSupported:
            caps = []
        return {"account": acct.ref, "provider": acct.provider, "capabilities": caps}

    try:
        return await run_in_threadpool(_caps)
    except AccountResolutionError as exc:
        raise HTTPException(status_code=400, detail=str(exc))


@app.get("/calendar")
async def calendar_list(
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    account: Optional[str] = None,
):
    key = cache_key(
        "calendar", account=account, days=days, from_date=from_date, to_date=to_date
    )
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry,
            account,
            CAP_CALENDAR,
            "list_events",
            days,
            from_date,
            to_date,
        ),
    )


@app.get("/calendar/{item_id}")
async def calendar_get(item_id: str, account: Optional[str] = None):
    """Get full details for a single calendar event."""
    return await safe_call_with_retry(account, CAP_CALENDAR, "get_event", item_id)


@app.post("/calendar")
async def calendar_create(payload: CalendarCreate, account: Optional[str] = None):
    data = payload.model_dump()
    attendees = data.get("attendees", [])
    required_attendees = data.get("required_attendees", [])
    optional_attendees = data.get("optional_attendees", [])

    if attendees or required_attendees or optional_attendees:
        # Use invite_event to create and send meeting invites
        return await safe_call_with_retry(
            account, CAP_CALENDAR_MEETINGS, "invite_event", data
        )
    # Simple event without attendees
    return await safe_call_with_retry(account, CAP_CALENDAR, "create_event", data)


class CalendarParse(BaseModel):
    """Request model for parsing natural language event descriptions."""

    input: str
    duration: int = 60
    location: Optional[str] = None


@app.post("/calendar/parse")
async def calendar_parse(payload: CalendarParse, account: Optional[str] = None):
    """Parse natural language event description into a calendar create payload.

    Parses inputs like:
    - "friday 2pm Team meeting with roman"
    - "tomorrow 10:30 for 2h Standup"
    - "jan 16 2pm-4pm Review"
    """
    from h8 import dateparser

    _ = current_account_email(account)  # Validate account exists

    # Parse attendees first
    remaining, attendee_aliases = dateparser.parse_attendees(payload.input)

    # Resolve aliases to emails
    attendees = []
    for alias in attendee_aliases:
        try:
            email = resolve_person_alias(alias)
            attendees.append(email)
        except ValueError:
            # Keep as-is if not a known alias (might be an email)
            attendees.append(alias)

    # Parse datetime from remaining text
    parsed = dateparser.parse_datetime(
        remaining,
        default_duration_minutes=payload.duration,
    )

    # Extract subject: whatever isn't date/time keywords
    # For now, look for quoted strings or capitalized phrases
    import re

    subject_match = re.search(r'"([^"]+)"', remaining)
    if subject_match:
        subject = subject_match.group(1)
    else:
        # Remove time/date keywords, month names, range separators, and "all day" from subject
        # Note: time patterns are ordered longest-first so "9:30am" is matched as one unit
        # (not split into "9", ":", "30am" leaving an orphan colon)
        cleaned = re.sub(
            r"\b(at|on|um|am|für|for|next|after|week|woche|nächste[rn]?|übernächsten?|uebernächsten?|uebernachsten?|today|tomorrow|morgen|"
            r"monday|tuesday|wednesday|thursday|friday|saturday|sunday|"
            r"montag|dienstag|mittwoch|donnerstag|freitag|samstag|sonntag|"
            r"january|february|march|april|may|june|july|august|september|october|november|december|"
            r"januar|februar|märz|maerz|mai|juni|juli|oktober|dezember|"
            r"jan|feb|mar|apr|jun|jul|aug|sep|sept|oct|okt|nov|dec|dez|"
            r"all\s*day|ganzt[aä]gig|ganztag|"
            r"till|until|through|bis|"
            r"\d{4}-\d{2}-\d{2}|\d{1,2}:\d{2}\s*(am|pm|uhr)?|\d{1,2}(am|pm|uhr)?)\b",
            "",
            remaining,
            flags=re.IGNORECASE,
        )
        # Clean up orphaned punctuation (colons, dashes) left after removing time tokens
        cleaned = re.sub(r"(?<!\w)[:;,\-]+(?!\w)", "", cleaned)
        cleaned = re.sub(r"\s+", " ", cleaned).strip()
        subject = cleaned if cleaned else "Event"

    # Build calendar create payload
    result = {
        "subject": subject,
        "start": parsed.start.isoformat(),
        "end": parsed.end.isoformat(),
    }

    if parsed.is_all_day:
        result["is_all_day"] = True

    if payload.location:
        result["location"] = payload.location

    if attendees:
        result["attendees"] = attendees

    return result


@app.delete("/calendar/{item_id}")
async def calendar_delete(
    item_id: str, changekey: Optional[str] = None, account: Optional[str] = None
):
    return await safe_call_with_retry(account, CAP_CALENDAR, "delete_event", item_id)


class CalendarCancel(BaseModel):
    """Request model for cancelling a meeting."""

    message: Optional[str] = None


@app.post("/calendar/{item_id}/cancel")
async def calendar_cancel(
    item_id: str, payload: CalendarCancel, account: Optional[str] = None
):
    """Cancel a calendar event and notify all attendees."""
    return await safe_call_with_retry(
        account, CAP_CALENDAR_MEETINGS, "cancel_event", item_id, payload.message
    )


@app.get("/calendar/search")
async def calendar_search(
    q: str,
    days: int = 90,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    limit: int = 50,
    account: Optional[str] = None,
):
    """Search calendar events by subject, location, or body content."""
    return await safe_call_with_retry(
        account, CAP_CALENDAR, "search_events", q, days, from_date, to_date, limit
    )


@app.post("/calendar/invite")
async def calendar_invite(payload: CalendarInvite, account: Optional[str] = None):
    """Create a calendar event and send meeting invites to attendees."""
    return await safe_call_with_retry(
        account, CAP_CALENDAR_MEETINGS, "invite_event", payload.model_dump()
    )


@app.get("/calendar/invites")
async def calendar_invites(
    limit: int = 50,
    account: Optional[str] = None,
):
    """List pending meeting invites from inbox."""
    return await safe_call_with_retry(
        account, CAP_CALENDAR_MEETINGS, "list_invites", limit
    )


@app.post("/calendar/{item_id}/rsvp")
async def calendar_rsvp(
    item_id: str, payload: CalendarRsvp, account: Optional[str] = None
):
    """Respond to a meeting invite (accept/decline/tentative)."""
    return await safe_call_with_retry(
        account, CAP_CALENDAR_MEETINGS, "rsvp_event", item_id, payload.response, payload.message
    )


@app.get("/mail")
async def mail_list(
    folder: str = "inbox",
    limit: int = 20,
    unread: bool = False,
    account: Optional[str] = None,
):
    key = cache_key("mail", account=account, folder=folder, limit=limit, unread=unread)
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry, account, CAP_MAIL, "list_messages", folder, limit, unread
        ),
    )


@app.get("/mail/search")
async def mail_search(
    q: str,
    folder: str = "inbox",
    limit: int = 50,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    account: Optional[str] = None,
):
    """Search messages by subject, sender, or body content.

    Supports:
    - Simple text: "meeting notes"
    - Field-specific: "subject:meeting" or "from:john@example.com"
    - OR queries: "meeting | standup" or "meeting OR standup"
    - Date filtering: from_date/to_date (ISO format YYYY-MM-DD)
    """
    return await safe_call_with_retry(
        account, CAP_MAIL, "search_messages", q, folder, limit, from_date, to_date
    )


@app.get("/mail/{item_id}")
async def mail_get(item_id: str, folder: str = "inbox", account: Optional[str] = None):
    return await safe_call_with_retry(account, CAP_MAIL, "get_message", item_id, folder)


class BatchGetRequest(BaseModel):
    """Request model for batch fetching messages."""

    ids: List[str]
    folder: str = "inbox"


@app.post("/mail/batch")
async def mail_batch_get(payload: BatchGetRequest, account: Optional[str] = None):
    """Fetch multiple messages by ID in a single request.

    This is much more efficient than making individual GET requests
    for each message when syncing.
    """
    return await safe_call_with_retry(
        account, CAP_MAIL, "batch_get_messages", payload.ids, payload.folder
    )


@app.post("/mail/send")
async def mail_send(payload: SendEmail, account: Optional[str] = None):
    return await safe_call_with_retry(
        account, CAP_MAIL_SEND, "send_message", payload.model_dump()
    )


@app.post("/mail/send-files")
async def mail_send_files(
    to: List[str] = Form(default_factory=list),
    cc: List[str] = Form(default_factory=list),
    subject: str = Form(""),
    body: str = Form(""),
    html: bool = Form(False),
    schedule_at: Optional[str] = Form(None),
    attachments: List[UploadFile] = File(default_factory=list),
    account: Optional[str] = None,
):
    """Send an email with file attachments using multipart/form-data.

    This complements ``/mail/send`` (JSON, supports base64 attachments) with an
    efficient streaming upload path for binary files. ``to`` and ``cc`` may each
    be repeated to supply multiple recipients.
    """
    att_specs = []
    for upload in attachments:
        content = await upload.read()
        att_specs.append(
            {"name": upload.filename or "attachment", "content": content}
        )

    message_data = {
        "to": to,
        "cc": cc,
        "subject": subject,
        "body": body,
        "html": html,
        "schedule_at": schedule_at,
        "attachments": att_specs,
    }
    return await safe_call_with_retry(
        account, CAP_MAIL_SEND, "send_message", message_data
    )


@app.post("/mail/fetch")
async def mail_fetch(payload: FetchMail, account: Optional[str] = None):
    return await safe_call_with_retry(
        account,
        CAP_MAIL,
        "fetch_messages",
        payload.folder,
        payload.output,
        payload.format,
        payload.limit,
    )


# Draft endpoints (ews-test-ahe)


@app.post("/mail/draft")
async def draft_save(payload: DraftSave, account: Optional[str] = None):
    """Save a new draft to the drafts folder."""
    return await safe_call_with_retry(
        account, CAP_MAIL, "save_draft", payload.model_dump()
    )


@app.put("/mail/draft/{item_id}")
async def draft_update(
    item_id: str, payload: DraftUpdate, account: Optional[str] = None
):
    """Update an existing draft."""
    update_data = {k: v for k, v in payload.model_dump().items() if v is not None}
    return await safe_call_with_retry(
        account, CAP_MAIL, "update_draft", item_id, update_data
    )


@app.delete("/mail/draft/{item_id}")
async def draft_delete(item_id: str, account: Optional[str] = None):
    """Delete a draft."""
    return await safe_call_with_retry(account, CAP_MAIL, "delete_draft", item_id)


@app.get("/mail/{item_id}/attachments")
async def mail_attachments_list(
    item_id: str, folder: str = "inbox", account: Optional[str] = None
):
    """List attachments for a message."""
    return await safe_call_with_retry(
        account, CAP_MAIL, "list_attachments", item_id, folder
    )


class AttachmentDownload(BaseModel):
    """Request model for downloading an attachment."""

    index: int
    output_path: str


@app.post("/mail/{item_id}/attachments/download")
async def mail_attachment_download(
    item_id: str,
    payload: AttachmentDownload,
    folder: str = "inbox",
    account: Optional[str] = None,
):
    """Download a specific attachment."""
    return await safe_call_with_retry(
        account,
        CAP_MAIL,
        "download_attachment",
        item_id,
        payload.index,
        payload.output_path,
        folder,
    )


# Message delete/move endpoints


@app.delete("/mail/{item_id}")
async def mail_delete(
    item_id: str,
    folder: str = "inbox",
    permanent: bool = False,
    account: Optional[str] = None,
):
    """Delete a message (move to trash or permanently delete)."""
    return await safe_call_with_retry(
        account, CAP_MAIL, "delete_message", item_id, folder, permanent
    )


class MailMove(BaseModel):
    """Request model for moving a message."""

    target_folder: str
    create_folder: bool = False


@app.post("/mail/{item_id}/move")
async def mail_move(
    item_id: str,
    payload: MailMove,
    folder: str = "inbox",
    account: Optional[str] = None,
):
    """Move a message to another folder."""
    return await safe_call_with_retry(
        account,
        CAP_MAIL_FOLDERS,
        "move_message",
        item_id,
        payload.target_folder,
        folder,
        payload.create_folder,
    )


@app.delete("/mail/folder/{folder_name}")
async def mail_empty_folder(
    folder_name: str,
    account: Optional[str] = None,
):
    """Empty a folder by permanently deleting all items."""
    return await safe_call_with_retry(
        account, CAP_MAIL_FOLDERS, "empty_folder", folder_name
    )


class MailBatchMoveOld(BaseModel):
    """Request model for moving old messages in bulk."""

    folder: str = "inbox"
    target_folder: str
    older_than_days: int = 7
    query: Optional[str] = None
    limit: int = 500
    create_folder: bool = True
    dry_run: bool = False


class MailBatchMark(BaseModel):
    """Request model for bulk mark read/unread."""

    folder: str = "inbox"
    read: bool
    ids: List[str] = Field(default_factory=list)
    older_than_days: Optional[int] = None
    query: Optional[str] = None
    limit: int = 500
    dry_run: bool = False

    @field_validator("ids", mode="before")
    @classmethod
    def _coerce_ids(cls, v):
        if v is None:
            return []
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


@app.post("/mail/move-old")
async def mail_move_old(payload: MailBatchMoveOld, account: Optional[str] = None):
    """Move old messages in bulk."""
    return await safe_call_with_retry(
        account,
        CAP_MAIL_FOLDERS,
        "batch_move_messages",
        payload.folder,
        payload.target_folder,
        payload.older_than_days,
        payload.query,
        payload.limit,
        payload.create_folder,
        payload.dry_run,
    )


@app.post("/mail/mark")
async def mail_mark_batch(payload: MailBatchMark, account: Optional[str] = None):
    """Mark messages as read/unread in bulk."""
    return await safe_call_with_retry(
        account,
        CAP_MAIL,
        "batch_mark_messages",
        payload.folder,
        payload.read,
        payload.ids,
        payload.older_than_days,
        payload.query,
        payload.limit,
        payload.dry_run,
    )


class MailSpam(BaseModel):
    """Request model for marking a message as spam."""

    is_spam: bool = True
    move: bool = True


@app.post("/mail/{item_id}/spam")
async def mail_mark_spam(
    item_id: str,
    payload: MailSpam,
    account: Optional[str] = None,
):
    """Mark a message as spam or not spam."""
    return await safe_call_with_retry(
        account, CAP_MAIL, "mark_as_spam", item_id, payload.is_spam, payload.move
    )


@app.get("/contacts")
async def contacts_list(
    limit: int = 100,
    search: Optional[str] = None,
    account: Optional[str] = None,
):
    key = cache_key("contacts", account=account, limit=limit, search=search)
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry, account, CAP_CONTACTS, "list_contacts", limit, search
        ),
    )


@app.get("/contacts/{item_id}")
async def contacts_get(item_id: str, account: Optional[str] = None):
    return await safe_call_with_retry(account, CAP_CONTACTS, "get_contact", item_id)


@app.post("/contacts")
async def contacts_create(payload: ContactCreate, account: Optional[str] = None):
    return await safe_call_with_retry(
        account, CAP_CONTACTS, "create_contact", payload.model_dump()
    )


@app.delete("/contacts/{item_id}")
async def contacts_delete(item_id: str, account: Optional[str] = None):
    return await safe_call_with_retry(account, CAP_CONTACTS, "delete_contact", item_id)


@app.put("/contacts/{item_id}")
async def contacts_update(
    item_id: str, payload: ContactUpdate, account: Optional[str] = None
):
    """Update an existing contact."""
    update_data = {k: v for k, v in payload.model_dump().items() if v is not None}
    return await safe_call_with_retry(
        account, CAP_CONTACTS, "update_contact", item_id, update_data
    )


@app.get("/free")
async def free_slots(
    weeks: int = 1,
    duration: int = 30,
    limit: Optional[int] = None,
    account: Optional[str] = None,
):
    key = cache_key("free", account=account, weeks=weeks, duration=duration, limit=limit)
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry,
            account,
            CAP_FREEBUSY_SELF,
            "find_free_slots",
            weeks,
            duration,
            limit,
        ),
    )


# People endpoints (view other people's calendars)


@app.get("/ppl/agenda")
async def ppl_agenda(
    person: str,
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    account: Optional[str] = None,
):
    """Get another person's calendar events (free/busy info)."""
    try:
        target_email = resolve_person_alias(person)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    # Validate the target email resolves to a real mailbox (trx-bf2h)
    is_valid = await safe_call_with_retry(
        account, CAP_GAL, "validate_email", target_email
    )
    if isinstance(is_valid, Response):
        return is_valid
    if not is_valid:
        raise HTTPException(
            status_code=404,
            detail=f"Email '{target_email}' does not resolve to a valid mailbox. "
            f"Use 'h8 addr resolve <query>' to search the directory.",
        )
    return await safe_call_with_retry(
        account,
        CAP_FREEBUSY_OTHERS,
        "get_person_agenda",
        target_email,
        days,
        from_date,
        to_date,
    )


@app.get("/ppl/free")
async def ppl_free(
    person: str,
    weeks: int = 1,
    duration: int = 30,
    limit: Optional[int] = None,
    account: Optional[str] = None,
):
    """Find free slots in another person's calendar."""
    try:
        target_email = resolve_person_alias(person)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    # Validate the target email resolves to a real mailbox (trx-bf2h)
    is_valid = await safe_call_with_retry(
        account, CAP_GAL, "validate_email", target_email
    )
    if isinstance(is_valid, Response):
        return is_valid
    if not is_valid:
        raise HTTPException(
            status_code=404,
            detail=f"Email '{target_email}' does not resolve to a valid mailbox. "
            f"Use 'h8 addr resolve <query>' to search the directory.",
        )
    return await safe_call_with_retry(
        account,
        CAP_FREEBUSY_OTHERS,
        "get_person_free_slots",
        target_email,
        weeks,
        duration,
        limit,
    )


class CommonFreeRequest(BaseModel):
    """Request model for finding common free slots."""

    people: List[str]
    weeks: int = 1
    duration: int = 30
    limit: Optional[int] = None


@app.post("/ppl/common")
async def ppl_common(payload: CommonFreeRequest, account: Optional[str] = None):
    """Find common free slots between multiple people."""
    try:
        target_emails = [resolve_person_alias(p) for p in payload.people]
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    if len(target_emails) < 2:
        raise HTTPException(
            status_code=400, detail="At least 2 people are required for common slots"
        )
    # Validate all target emails resolve to real mailboxes (trx-bf2h)
    invalid_emails = []
    for target_email in target_emails:
        is_valid = await safe_call_with_retry(
            account, CAP_GAL, "validate_email", target_email
        )
        if isinstance(is_valid, Response):
            return is_valid
        if not is_valid:
            invalid_emails.append(target_email)
    if invalid_emails:
        raise HTTPException(
            status_code=404,
            detail=f"The following email(s) do not resolve to valid mailboxes: "
            f"{', '.join(invalid_emails)}. "
            f"Use 'h8 addr resolve <query>' to search the directory.",
        )
    return await safe_call_with_retry(
        account,
        CAP_FREEBUSY_OTHERS,
        "find_common_free_slots",
        target_emails,
        payload.weeks,
        payload.duration,
        payload.limit,
    )


# Resource group endpoints


class ResourceItem(BaseModel):
    """A single resource in a group."""

    alias: str
    email: str
    desc: Optional[str] = None


class ResourceFreeRequest(BaseModel):
    """Request model for querying resource availability."""

    resources: List[ResourceItem]
    from_date: Optional[str] = None
    to_date: Optional[str] = None
    days: int = 1
    start_hour: Optional[int] = None
    end_hour: Optional[int] = None


class ResourceFreeWindowRequest(BaseModel):
    """Request model for checking resource availability in a specific time window."""

    resources: List[ResourceItem]
    from_date: str
    to_date: str


class ResourceAgendaRequest(BaseModel):
    """Request model for querying resource bookings."""

    resources: List[ResourceItem]
    from_date: Optional[str] = None
    to_date: Optional[str] = None
    days: int = 1


@app.post("/resource/free")
async def resource_free(payload: ResourceFreeRequest, account: Optional[str] = None):
    """Check free slots for each resource in a group."""
    resource_list = [r.model_dump() for r in payload.resources]
    return await safe_call_with_retry(
        account,
        CAP_RESOURCES,
        "resource_free",
        resource_list,
        payload.from_date,
        payload.to_date,
        payload.days,
        payload.start_hour,
        payload.end_hour,
    )


@app.post("/resource/free-window")
async def resource_free_window(
    payload: ResourceFreeWindowRequest, account: Optional[str] = None
):
    """Check if each resource is available during a specific time window."""
    resource_list = [r.model_dump() for r in payload.resources]
    return await safe_call_with_retry(
        account,
        CAP_RESOURCES,
        "resource_free_window",
        resource_list,
        payload.from_date,
        payload.to_date,
    )


@app.post("/resource/agenda")
async def resource_agenda(
    payload: ResourceAgendaRequest, account: Optional[str] = None
):
    """Get bookings/events for each resource in a group."""
    resource_list = [r.model_dump() for r in payload.resources]
    return await safe_call_with_retry(
        account,
        CAP_RESOURCES,
        "resource_agenda",
        resource_list,
        payload.from_date,
        payload.to_date,
        payload.days,
    )


# Address / GAL resolve endpoints


@app.get("/addr/resolve")
async def addr_resolve(
    q: str,
    account: Optional[str] = None,
):
    """Resolve a name or email against the Global Address List (GAL).

    Uses EWS ResolveNames to find mailboxes including resource rooms,
    equipment, and distribution lists.
    """
    return await safe_call_with_retry(account, CAP_GAL, "resolve_names", q)


@app.get("/addr/validate")
async def addr_validate(
    email_addr: str,
    account: Optional[str] = None,
):
    """Validate whether an email address resolves to a real mailbox.

    Returns {"valid": true/false, "email": "..."}.
    """
    is_valid = await safe_call_with_retry(
        account, CAP_GAL, "validate_email", email_addr
    )
    if isinstance(is_valid, Response):
        return is_valid
    return {"valid": is_valid, "email": email_addr}


# === Trip / Routing ===


class GeocodeRequest(BaseModel):
    query: str
    country: Optional[str] = None  # ISO 3166-1 alpha-2 to bias results; None = worldwide


class RouteRequest(BaseModel):
    origin_lat: float
    origin_lon: float
    dest_lat: float
    dest_lon: float
    mode: str = "car"  # "car" or "transit"
    origin_station: Optional[str] = None  # For transit routing
    dest_station: Optional[str] = None  # For transit routing
    transit_provider: str = "db"  # "db", "sbb", etc.
    departure: Optional[str] = None  # ISO datetime
    arrival: Optional[str] = None  # ISO datetime (find connections arriving by this time)


@app.post("/trip/geocode")
async def trip_geocode(payload: GeocodeRequest):
    """Geocode an address or place name to coordinates (worldwide)."""
    from h8 import routing

    result = await routing.geocode(payload.query, payload.country)
    if not result:
        raise HTTPException(404, f"Could not geocode: {payload.query}")
    return {
        "lat": result.lat,
        "lon": result.lon,
        "display_name": result.display_name,
        "address": result.address,
    }


@app.post("/trip/route")
async def trip_route(payload: RouteRequest):
    """Calculate a route between two points (worldwide)."""
    from h8 import routing
    from datetime import datetime as dt

    departure = None
    arrival = None
    if payload.departure:
        try:
            departure = dt.fromisoformat(payload.departure)
        except ValueError:
            raise HTTPException(400, f"Invalid departure datetime: {payload.departure}")
    if payload.arrival:
        try:
            arrival = dt.fromisoformat(payload.arrival)
        except ValueError:
            raise HTTPException(400, f"Invalid arrival datetime: {payload.arrival}")

    result = await routing.calculate_route(
        origin_lat=payload.origin_lat,
        origin_lon=payload.origin_lon,
        dest_lat=payload.dest_lat,
        dest_lon=payload.dest_lon,
        mode=payload.mode,
        origin_station=payload.origin_station,
        dest_station=payload.dest_station,
        transit_provider=payload.transit_provider,
        departure=departure,
        arrival=arrival,
    )
    if not result:
        raise HTTPException(
            502, f"Routing failed for mode={payload.mode}"
        )

    response: dict = {
        "mode": result.mode,
        "duration_minutes": result.duration_minutes,
        "distance_km": result.distance_km,
    }
    if result.car_route:
        response["car"] = {
            "duration_seconds": result.car_route.duration_seconds,
            "distance_meters": result.car_route.distance_meters,
        }
    if result.transit_journeys:
        response["transit_journeys"] = [
            {
                "provider": j.provider,
                "total_duration_minutes": j.total_duration_minutes,
                "departure_time": j.departure_time,
                "arrival_time": j.arrival_time,
                "changes": j.changes,
                "legs": [
                    {
                        "line": leg.line,
                        "mode": leg.mode,
                        "walking": leg.walking,
                        "departure_station": leg.departure_station,
                        "arrival_station": leg.arrival_station,
                        "departure_time": leg.departure_time,
                        "arrival_time": leg.arrival_time,
                        "duration_minutes": leg.duration_minutes,
                        "platform": leg.platform,
                        "arrival_platform": leg.arrival_platform,
                        "distance_meters": leg.distance_meters,
                    }
                    for leg in j.legs
                ],
            }
            for j in result.transit_journeys
        ]
    return response


# === Rules and OOF Endpoints ===


@app.get("/rules")
async def rules_list(account: Optional[str] = None):
    """List all inbox rules."""
    return await safe_call_with_retry(account, CAP_RULES, "list_rules")


@app.get("/rules/{rule_id}")
async def rules_get(rule_id: str, account: Optional[str] = None):
    """Get a specific rule by ID."""
    result = await safe_call_with_retry(account, CAP_RULES, "get_rule", rule_id)
    if isinstance(result, Response):
        return result
    if result is None:
        raise HTTPException(status_code=404, detail=f"Rule '{rule_id}' not found")
    return result


@app.post("/rules")
async def rules_create(payload: RuleCreate, account: Optional[str] = None):
    """Create a new inbox rule."""
    return await safe_call_with_retry(
        account,
        CAP_RULES,
        "create_rule",
        payload.display_name,
        payload.priority,
        payload.is_enabled,
        payload.conditions,
        payload.actions,
    )


@app.put("/rules/{rule_id}")
async def rules_update(
    rule_id: str, payload: RuleUpdate, account: Optional[str] = None
):
    """Update an existing inbox rule."""
    return await safe_call_with_retry(
        account,
        CAP_RULES,
        "update_rule",
        rule_id,
        payload.display_name,
        payload.priority,
        payload.is_enabled,
        payload.conditions,
        payload.actions,
    )


@app.post("/rules/{rule_id}/enable")
async def rules_enable(rule_id: str, account: Optional[str] = None):
    """Enable an inbox rule."""
    return await safe_call_with_retry(account, CAP_RULES, "enable_rule", rule_id)


@app.post("/rules/{rule_id}/disable")
async def rules_disable(rule_id: str, account: Optional[str] = None):
    """Disable an inbox rule."""
    return await safe_call_with_retry(account, CAP_RULES, "disable_rule", rule_id)


@app.delete("/rules/{rule_id}")
async def rules_delete(rule_id: str, account: Optional[str] = None):
    """Delete an inbox rule."""
    result = await safe_call_with_retry(account, CAP_RULES, "delete_rule", rule_id)
    if isinstance(result, Response):
        return result
    return {"success": True, "id": rule_id}


@app.get("/oof")
async def oof_get(account: Optional[str] = None):
    """Get Out-of-Office settings."""
    return await safe_call_with_retry(account, CAP_OOF, "get_oof_settings")


@app.put("/oof")
async def oof_set(payload: OofSettings, account: Optional[str] = None):
    """Set Out-of-Office settings."""
    return await safe_call_with_retry(
        account,
        CAP_OOF,
        "set_oof_settings",
        payload.state,
        payload.external_audience,
        payload.start,
        payload.end,
        payload.internal_reply,
        payload.external_reply,
    )


@app.post("/oof/enable")
async def oof_enable(payload: OofEnable, account: Optional[str] = None):
    """Enable Out-of-Office (immediate, not scheduled)."""
    return await safe_call_with_retry(
        account,
        CAP_OOF,
        "enable_oof",
        payload.internal_reply,
        payload.external_reply,
        payload.external_audience,
    )


@app.post("/oof/schedule")
async def oof_schedule(payload: OofSchedule, account: Optional[str] = None):
    """Schedule Out-of-Office for a future period."""
    return await safe_call_with_retry(
        account,
        CAP_OOF,
        "schedule_oof",
        payload.start,
        payload.end,
        payload.internal_reply,
        payload.external_reply,
        payload.external_audience,
    )


@app.post("/oof/disable")
async def oof_disable(account: Optional[str] = None):
    """Disable Out-of-Office."""
    return await safe_call_with_retry(account, CAP_OOF, "disable_oof")


# === Unsubscribe Endpoints ===


class UnsubscribeScan(BaseModel):
    """Request model for scanning messages for unsubscribe links."""

    folder: str = "inbox"
    sender: Optional[str] = None
    search: Optional[str] = None
    limit: int = 50
    safe_senders: List[str] = Field(default_factory=list)
    blocked_patterns: List[str] = Field(default_factory=list)

    @field_validator("safe_senders", "blocked_patterns", mode="before")
    @classmethod
    def _coerce_list(cls, v):
        if v is None:
            return []
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


class UnsubscribeExecute(BaseModel):
    """Request model for executing unsubscribes."""

    item_ids: List[str]
    safe_senders: List[str] = Field(default_factory=list)
    blocked_patterns: List[str] = Field(default_factory=list)
    trusted_domains: List[str] = Field(default_factory=list)
    rate_limit_seconds: float = 2.0

    @field_validator(
        "item_ids", "safe_senders", "blocked_patterns", "trusted_domains",
        mode="before",
    )
    @classmethod
    def _coerce_list(cls, v):
        if v is None:
            return []
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("must be a string or list of strings")


@app.post("/mail/unsubscribe/scan")
async def mail_unsubscribe_scan(
    payload: UnsubscribeScan, account: Optional[str] = None
):
    """Scan messages for unsubscribe links (dry run).

    Returns a list of messages with discovered unsubscribe links.
    Does NOT visit any URLs - safe to call repeatedly.
    """
    return await safe_call_with_retry(
        account,
        CAP_MAIL,
        "scan_unsubscribe",
        payload.folder,
        payload.sender,
        payload.search,
        payload.limit,
        payload.safe_senders,
        payload.blocked_patterns,
    )


@app.post("/mail/unsubscribe/execute")
async def mail_unsubscribe_execute(
    payload: UnsubscribeExecute, account: Optional[str] = None
):
    """Execute unsubscribe for the given message IDs.

    Visits unsubscribe URLs and reports results.
    This actually performs the unsubscribe action.
    """
    return await safe_call_with_retry(
        account,
        CAP_MAIL,
        "execute_unsubscribe",
        payload.item_ids,
        payload.safe_senders,
        payload.blocked_patterns,
        payload.trusted_domains,
        payload.rate_limit_seconds,
    )


# === Auth endpoints (OAuth login/status/logout) ===
#
# These /auth/* endpoints require the ``admin:write`` scope (wired via
# ROUTE_SCOPES / _apply_route_scopes below).


class AuthLoginRequest(BaseModel):
    """Request model for starting a login flow."""

    account: str


class AuthFinishRequest(BaseModel):
    """Request model for finishing the Google URL-paste flow."""

    redirect_url: str


class AuthLogoutRequest(BaseModel):
    """Request model for logging an account out."""

    account: str


def _list_account_configs() -> List[Any]:
    """Return every configured account plus the legacy default (deduped by ref).

    Runs off the event loop (config read only). Order: ``[accounts.*]`` tables
    first, then the legacy top-level ``account`` if it is not already listed.
    """
    from h8.config import get_config

    cfg = get_config()
    result: List[Any] = []
    seen: set = set()

    tables = cfg.get("accounts")
    if isinstance(tables, dict):
        for alias in tables:
            try:
                acct = resolve_account(alias)
            except AccountResolutionError:
                continue
            if acct.ref in seen:
                continue
            seen.add(acct.ref)
            result.append(acct)

    if cfg.get("account"):
        try:
            acct = resolve_account(None)
        except AccountResolutionError:
            acct = None
        if acct is not None and acct.ref not in seen:
            seen.add(acct.ref)
            result.append(acct)

    return result


def _auth_accounts() -> List[dict]:
    """Build the /auth/accounts payload (login state via the OAuth facade)."""
    from h8 import oauth

    out: List[dict] = []
    for acct in _list_account_configs():
        status = oauth.login_status(acct)
        out.append(
            {
                "alias": acct.alias,
                "email": acct.email,
                "provider": acct.provider,
                "logged_in": status.get("logged_in", False),
                "expires_at": status.get("expires_at"),
            }
        )
    return out


def _auth_start_login(account_ref: str) -> dict:
    """Start a login flow for ``account_ref`` (device-code or google URL)."""
    from h8 import oauth
    from h8.oauth import LoginRequired

    acct = resolve_account(account_ref)
    try:
        if acct.provider in ("ews", "graph"):
            device = oauth.start_device_login(acct)
            return {
                "flow": "device_code",
                "session_id": device.session_id,
                "verification_url": device.verification_url,
                "user_code": device.user_code,
            }
        if acct.provider == "google":
            session = oauth.google.start_login(acct, headless=True)
            return {
                "flow": "auth_url",
                "session_id": session.session_id,
                "auth_url": session.auth_url,
            }
    except LoginRequired as exc:
        raise HTTPException(status_code=400, detail=str(exc))
    raise HTTPException(
        status_code=400,
        detail=f"Unsupported provider '{acct.provider}' for login",
    )


def _normalize_poll(status: str) -> dict:
    """Turn a provider poll string into ``{"status", "detail"?}``."""
    if status.startswith("error"):
        _, _, detail = status.partition(":")
        return {"status": "error", "detail": detail.strip() or "unknown error"}
    return {"status": status}


def _auth_poll_login(session_id: str) -> dict:
    """Poll a login session across both providers and normalize the result.

    On completion, drop cached backends so the next request rebuilds with the
    freshly stored credentials. (The registry only exposes a full cache clear;
    backends rebuild lazily and tokens are cached in the store, so this is cheap.)
    """
    from h8 import oauth
    from h8.providers import registry

    status = oauth.poll_device_login(session_id)
    if status == "error: unknown session":
        status = oauth.poll_login(session_id)  # google session?

    if status == "done":
        registry.clear_cache()
    return _normalize_poll(status)


def _auth_finish_login(session_id: str, redirect_url: str) -> dict:
    """Complete the Google URL-paste flow and clear cached backends."""
    from h8 import oauth
    from h8.oauth import LoginRequired
    from h8.providers import registry

    try:
        oauth.finish_url_login(session_id, redirect_url)
    except LoginRequired as exc:
        raise HTTPException(status_code=400, detail=str(exc))
    registry.clear_cache()
    return {"status": "done"}


def _auth_logout(account_ref: str) -> dict:
    """Delete stored OAuth state for the account and clear cached backends."""
    from h8 import oauth
    from h8.providers import registry

    acct = resolve_account(account_ref)
    oauth.logout(acct)
    registry.clear_cache()
    return {"status": "ok", "account": acct.ref}


@app.get("/auth/accounts")
async def auth_accounts():
    """List every configured account with its login state (scope: admin:write)."""
    return await run_in_threadpool(_auth_accounts)


@app.post("/auth/login")
async def auth_login(payload: AuthLoginRequest):
    """Start a login flow for an account (device-code or google auth URL)."""
    try:
        return await run_in_threadpool(_auth_start_login, payload.account)
    except AccountResolutionError as exc:
        raise HTTPException(status_code=400, detail=str(exc))


@app.get("/auth/login/{session_id}")
async def auth_login_status(session_id: str):
    """Poll a login session: ``{"status": pending|done|error, "detail"?}``."""
    return await run_in_threadpool(_auth_poll_login, session_id)


@app.post("/auth/login/{session_id}/finish")
async def auth_login_finish(session_id: str, payload: AuthFinishRequest):
    """Finish the Google URL-paste flow with the pasted redirect URL."""
    return await run_in_threadpool(
        _auth_finish_login, session_id, payload.redirect_url
    )


@app.post("/auth/logout")
async def auth_logout(payload: AuthLogoutRequest):
    """Delete stored OAuth credentials for an account."""
    try:
        return await run_in_threadpool(_auth_logout, payload.account)
    except AccountResolutionError as exc:
        raise HTTPException(status_code=400, detail=str(exc))


# === Key management endpoints (scope: keys:read / keys:write) ===


class KeyCreate(BaseModel):
    """Request model for creating an API key."""

    name: str
    scopes: List[str]
    accounts: Optional[List[str]] = None

    @field_validator("scopes", mode="before")
    @classmethod
    def _coerce_scopes(cls, v):
        if isinstance(v, str):
            return [v]
        if isinstance(v, (list, tuple)):
            return list(v)
        raise ValueError("scopes must be a string or list of strings")


@app.get("/keys")
async def keys_list():
    """List all API keys (never exposes the token hash)."""
    return await run_in_threadpool(
        lambda: [public_key_view(k) for k in key_store.list_keys()]
    )


@app.post("/keys")
async def keys_create(payload: KeyCreate):
    """Create an API key. The raw ``token`` is returned exactly once."""
    try:
        security.validate_scopes(payload.scopes)
    except ValueError as exc:
        raise HTTPException(status_code=422, detail=str(exc))

    def _create() -> dict:
        record, token = key_store.create_key(
            payload.name, payload.scopes, payload.accounts
        )
        view = public_key_view(record)
        view["token"] = token
        return view

    return await run_in_threadpool(_create)


@app.delete("/keys/{key_id}")
async def keys_delete(key_id: str):
    """Revoke (disable) an API key. Revoking the last enabled key is allowed."""

    def _revoke() -> dict:
        if not key_store.revoke(key_id):
            raise HTTPException(
                status_code=404, detail=f"No enabled key with id '{key_id}'"
            )
        return {"status": "revoked", "id": key_id}

    return await run_in_threadpool(_revoke)


# === Server-side auth CLI (`h8-service auth ...`) ===
#
# Runs the OAuth flows inline via direct oauth calls -- works without the HTTP
# service running.


def _cli_auth_login(account_ref: Optional[str]) -> int:
    from h8 import oauth
    from h8.oauth import LoginRequired
    from h8.providers import registry

    try:
        acct = resolve_account(account_ref)
    except AccountResolutionError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    try:
        if acct.provider in ("ews", "graph"):
            device = oauth.start_device_login(acct)
            print(
                f"Visit {device.verification_url} and enter code "
                f"{device.user_code}"
            )
            print("Waiting for you to complete sign-in...")
            while True:
                status = oauth.poll_device_login(device.session_id)
                if status == "done":
                    break
                if status.startswith("error"):
                    print(status, file=sys.stderr)
                    return 1
                time.sleep(3)
        elif acct.provider == "google":
            session = oauth.google.start_login(acct, headless=True)
            print("Open this URL in a browser and authorize access:")
            print(f"  {session.auth_url}")
            redirect_url = input(
                "Paste the full redirect URL you were sent to: "
            ).strip()
            oauth.finish_url_login(session.session_id, redirect_url)
        else:
            print(
                f"error: unsupported provider '{acct.provider}'",
                file=sys.stderr,
            )
            return 1
    except LoginRequired as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    registry.clear_cache()
    print(f"Logged in as {acct.email} ({acct.ref}).")
    return 0


def _cli_auth_status(account_ref: Optional[str]) -> int:
    from h8 import oauth
    from datetime import datetime, timezone as _tz

    if account_ref:
        try:
            accounts = [resolve_account(account_ref)]
        except AccountResolutionError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
    else:
        accounts = _list_account_configs()

    if not accounts:
        print("No accounts configured.")
        return 0

    rows = []
    for acct in accounts:
        status = oauth.login_status(acct)
        expires_at = status.get("expires_at")
        if expires_at:
            expires = datetime.fromtimestamp(expires_at, _tz.utc).strftime(
                "%Y-%m-%d %H:%M UTC"
            )
        else:
            expires = "-"
        rows.append(
            (
                acct.ref,
                acct.provider,
                "yes" if status.get("logged_in") else "no",
                expires,
            )
        )

    headers = ("ACCOUNT", "PROVIDER", "LOGGED IN", "EXPIRES")
    widths = [
        max(len(headers[i]), *(len(r[i]) for r in rows)) for i in range(len(headers))
    ]
    line = "  ".join(h.ljust(widths[i]) for i, h in enumerate(headers))
    print(line)
    for row in rows:
        print("  ".join(row[i].ljust(widths[i]) for i in range(len(row))))
    return 0


def _cli_auth_logout(account_ref: Optional[str]) -> int:
    from h8 import oauth
    from h8.providers import registry

    try:
        acct = resolve_account(account_ref)
    except AccountResolutionError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    oauth.logout(acct)
    registry.clear_cache()
    print(f"Logged out {acct.ref}.")
    return 0


def _cli_keys_create(name: str, scopes: List[str], accounts: Optional[List[str]]) -> int:
    """Create a key directly on the KeyStore and print the raw token once."""
    try:
        security.validate_scopes(scopes)
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    record, token = key_store.create_key(name, scopes, accounts)
    print(f"Created key {record['id']} ({record['name']})")
    print(f"  scopes:   {', '.join(record['scopes'])}")
    print(f"  accounts: {record['accounts'] if record['accounts'] else 'any'}")
    print()
    print("Token (shown once -- store it now):")
    print(f"  {token}")
    return 0


def _cli_keys_list() -> int:
    """Print all keys (id, name, scopes, accounts, state) from the KeyStore."""
    keys = key_store.list_keys()
    if not keys:
        print("No keys defined.")
        return 0
    rows = []
    for key in keys:
        view = public_key_view(key)
        rows.append(
            (
                view["id"] or "",
                view["name"] or "",
                ",".join(view["scopes"]),
                ",".join(view["accounts"]) if view["accounts"] else "any",
                "disabled" if view["disabled"] else "enabled",
            )
        )
    headers = ("ID", "NAME", "SCOPES", "ACCOUNTS", "STATE")
    widths = [
        max(len(headers[i]), *(len(r[i]) for r in rows)) for i in range(len(headers))
    ]
    print("  ".join(h.ljust(widths[i]) for i, h in enumerate(headers)))
    for row in rows:
        print("  ".join(row[i].ljust(widths[i]) for i in range(len(row))))
    return 0


def _cli_keys_revoke(key_id: str) -> int:
    """Disable a key by id directly on the KeyStore."""
    if key_store.revoke(key_id):
        print(f"Revoked key {key_id}.")
        return 0
    print(f"error: no enabled key with id '{key_id}'", file=sys.stderr)
    return 1


def _keys_cli(argv: List[str]) -> int:
    """Handle ``h8-service keys <create|list|revoke>`` against the KeyStore."""
    import argparse

    parser = argparse.ArgumentParser(
        prog="h8-service keys",
        description="Manage h8 service API keys (operates directly on the key store)",
    )
    sub = parser.add_subparsers(dest="action", required=True)

    p_create = sub.add_parser("create", help="Create a new API key")
    p_create.add_argument("--name", required=True, help="Human-readable key name")
    p_create.add_argument(
        "--scopes",
        required=True,
        help="Comma-separated scopes, e.g. 'mail:read,calendar:read'",
    )
    p_create.add_argument(
        "--accounts",
        default=None,
        help="Comma-separated account restriction (default: any account)",
    )

    sub.add_parser("list", help="List API keys")

    p_revoke = sub.add_parser("revoke", help="Revoke (disable) a key by id")
    p_revoke.add_argument("id", help="Key id to revoke")

    args = parser.parse_args(argv)

    if args.action == "create":
        scopes = [s.strip() for s in args.scopes.split(",") if s.strip()]
        accounts = (
            [a.strip() for a in args.accounts.split(",") if a.strip()]
            if args.accounts
            else None
        )
        return _cli_keys_create(args.name, scopes, accounts)
    if args.action == "list":
        return _cli_keys_list()
    if args.action == "revoke":
        return _cli_keys_revoke(args.id)
    return 2


def _auth_cli(argv: List[str]) -> int:
    """Handle ``h8-service auth <login|status|logout> [account]``."""
    import argparse

    parser = argparse.ArgumentParser(
        prog="h8-service auth",
        description="Manage OAuth credentials for h8 accounts",
    )
    sub = parser.add_subparsers(dest="action", required=True)
    for action in ("login", "status", "logout"):
        p = sub.add_parser(action, help=f"{action} an account")
        p.add_argument(
            "account",
            nargs="?",
            default=None,
            help="Account alias or email (default: configured default account)",
        )
    args = parser.parse_args(argv)

    if args.action == "login":
        return _cli_auth_login(args.account)
    if args.action == "status":
        return _cli_auth_status(args.account)
    if args.action == "logout":
        return _cli_auth_logout(args.account)
    return 2


# Attach scope enforcement to every route in ROUTE_SCOPES. Must run after all
# @app decorators above have registered their routes.
_apply_route_scopes()


def main() -> None:
    argv = sys.argv[1:]
    if argv and argv[0] == "auth":
        raise SystemExit(_auth_cli(argv[1:]))
    if argv and argv[0] == "keys":
        raise SystemExit(_keys_cli(argv[1:]))
    uvicorn.run(
        "h8.service:app",
        host=DEFAULT_HOST,
        port=DEFAULT_PORT,
        log_level="info",
    )


if __name__ == "__main__":
    main()
