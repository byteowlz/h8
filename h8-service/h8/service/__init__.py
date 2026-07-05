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
import logging
import os
import time
from contextlib import asynccontextmanager
from functools import partial
from typing import Any, Dict, List, Optional

import uvicorn
from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import JSONResponse, Response
from pydantic import BaseModel, Field, field_validator

from h8 import auth
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
    # Configure GPG for headless environments before any token operations (oama).
    with contextlib.suppress(Exception):
        await run_in_threadpool(auth.ensure_gpg_headless)
    # Refresh tokens immediately on startup to ensure we have valid tokens.
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


def main() -> None:
    uvicorn.run(
        "h8.service:app",
        host=DEFAULT_HOST,
        port=DEFAULT_PORT,
        log_level="info",
    )


if __name__ == "__main__":
    main()
