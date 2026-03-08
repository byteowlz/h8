"""
Background EWS service that uses the existing Python exchangelib logic and exposes
cached endpoints for the Rust CLI.

Features:
- Automatic token refresh when tokens expire
- Retry with fresh token on UnauthorizedError
- Draft management endpoints
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import time
from functools import partial
from typing import Any, Dict, List, Optional

import uvicorn
from fastapi import FastAPI
from fastapi import HTTPException
from fastapi.concurrency import run_in_threadpool
from pydantic import BaseModel, Field, field_validator

from h8 import auth, calendar, contacts, free, mail, people, resolve, resources, rules_oof, unsubscribe
from h8.config import get_config, resolve_person_alias
from exchangelib.errors import UnauthorizedError, ErrorServerBusy

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
    """Get the account email, using config default if not specified."""
    config = get_config()
    account = requested or config.get("account")
    if not account:
        raise HTTPException(
            status_code=400, detail="No account specified and no default configured"
        )
    return account


def current_ews_account(requested: Optional[str]):
    """Get an authenticated EWS account."""
    email = current_account_email(requested)
    return auth.get_account(email)


def refresh_ews_account(requested: Optional[str]):
    """Force refresh an EWS account's authentication."""
    email = current_account_email(requested)
    return auth.refresh_account(email)


async def safe_call_with_retry(func, account_email: str, *args, **kwargs):
    """
    Execute a function with automatic retry on UnauthorizedError or ErrorServerBusy.

    If an UnauthorizedError occurs, renews token via oama and retries once.
    If ErrorServerBusy occurs, retries with exponential backoff.
    """
    max_retries = 3
    base_delay = 2  # seconds

    for attempt in range(max_retries):
        try:
            return await run_in_threadpool(func, *args, **kwargs)
        except UnauthorizedError as exc:
            log.warning(
                "UnauthorizedError for %s, renewing token and retrying...", account_email
            )
            try:
                # Renew via oama and refresh the account token
                new_account = auth.renew_and_refresh_account(account_email)
                # Replace the account argument if it's the first positional arg
                if args and hasattr(args[0], "primary_smtp_address"):
                    args = (new_account,) + args[1:]
                return await run_in_threadpool(func, *args, **kwargs)
            except UnauthorizedError:
                log.error("Retry failed with UnauthorizedError for %s", account_email)
                raise HTTPException(status_code=401, detail=f"Authentication failed: {exc}")
            except Exception as retry_exc:
                log.error("Retry failed with error: %s", retry_exc)
                raise HTTPException(status_code=500, detail=str(retry_exc))
        except ErrorServerBusy as exc:
            if attempt < max_retries - 1:
                delay = base_delay * (2 ** attempt)
                log.warning(
                    "Server busy for %s, retrying in %ds (attempt %d/%d)...",
                    account_email, delay, attempt + 1, max_retries
                )
                await asyncio.sleep(delay)
                continue
            else:
                log.error("Server busy retries exhausted for %s", account_email)
                raise HTTPException(status_code=503, detail=f"Exchange server busy, please try again later: {exc}")
        except Exception as exc:
            log.error("Error in safe_call: %s", exc)
            raise HTTPException(status_code=500, detail=str(exc))


async def safe_call(func, *args, **kwargs):
    """Execute a function, converting exceptions to HTTP errors."""
    try:
        return await run_in_threadpool(func, *args, **kwargs)
    except UnauthorizedError as exc:
        raise HTTPException(status_code=401, detail=str(exc))
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))


app = FastAPI(title="h8-service", version="0.2.0")
cache: Dict[str, CacheEntry] = {}
cache_lock = asyncio.Lock()


def cache_key(prefix: str, **params: Any) -> str:
    parts = [prefix] + [f"{k}={v}" for k, v in sorted(params.items())]
    return "|".join(parts)


async def get_or_set(key: str, producer, account_email: Optional[str] = None):
    """Get cached value or produce and cache a new one."""
    async with cache_lock:
        entry = cache.get(key)
        if entry and entry.fresh():
            log.debug("cache hit: %s", key)
            return entry.data
    log.debug("cache miss: %s", key)
    try:
        data = await producer()
    except UnauthorizedError as exc:
        if account_email:
            log.warning(
                "UnauthorizedError during cache fill for %s, renewing token...",
                account_email,
            )
            auth.renew_and_refresh_account(account_email)
        raise HTTPException(status_code=401, detail=str(exc))
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))
    async with cache_lock:
        cache[key] = CacheEntry(data=data, ts=time.time())
    return data


async def refresh_defaults():
    """Refresh default cached queries for the configured account."""
    try:
        email = current_account_email(None)
        account = auth.get_account(email)
    except Exception as e:
        log.warning("Could not get account for default refresh: %s", e)
        return

    try:
        await get_or_set(
            cache_key("calendar", account=account, days=7),
            partial(
                run_in_threadpool,
                calendar.list_events,
                account,
                7,
                None,
                None,
            ),
            email,
        )
        await get_or_set(
            cache_key("mail", account=account, folder="inbox", limit=20, unread=False),
            partial(
                run_in_threadpool,
                mail.list_messages,
                account,
                "inbox",
                20,
                False,
            ),
            email,
        )
        await get_or_set(
            cache_key("contacts", account=account, limit=100, search=None),
            partial(
                run_in_threadpool,
                contacts.list_contacts,
                account,
                100,
                None,
            ),
            email,
        )
        await get_or_set(
            cache_key("free", account=account, weeks=1, duration=30, limit=None),
            partial(
                run_in_threadpool,
                free.find_free_slots,
                account,
                1,
                30,
                None,
            ),
            email,
        )
    except Exception:
        log.exception("default refresh failed")


async def refresh_tokens():
    """Periodically refresh authentication tokens before they expire.

    This proactively calls oama renew to get a fresh token before the current
    one expires, preventing authentication failures.
    """
    try:
        email = current_account_email(None)
        log.info("Proactively renewing and refreshing token for %s", email)
        # Call oama renew first to ensure we get a fresh token
        auth.renew_and_refresh_account(email)
    except Exception as e:
        log.warning("Token refresh failed: %s", e)


@app.on_event("startup")
async def startup() -> None:
    level = getattr(logging, LOG_LEVEL, logging.INFO)
    logging.basicConfig(level=level)
    # Refresh tokens immediately on startup to ensure we have valid tokens
    await refresh_tokens()
    app.state.refresh_task = asyncio.create_task(_refresh_loop())
    app.state.token_refresh_task = asyncio.create_task(_token_refresh_loop())
    log.info("h8-service started on %s:%s", DEFAULT_HOST, DEFAULT_PORT)


@app.on_event("shutdown")
async def shutdown() -> None:
    for task_name in ["refresh_task", "token_refresh_task"]:
        task = getattr(app.state, task_name, None)
        if task:
            task.cancel()
            with contextlib.suppress(Exception):
                await task
    log.info("h8-service shutdown")


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
    cache_info = auth.get_account_manager().get_cache_info()
    return {
        "status": "ok",
        "accounts": cache_info,
    }


@app.get("/calendar")
async def calendar_list(
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    account: Optional[str] = None,
):
    email = current_account_email(account)
    acct = auth.get_account(email)
    key = cache_key(
        "calendar", account=acct, days=days, from_date=from_date, to_date=to_date
    )
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry,
            calendar.list_events,
            email,
            acct,
            days,
            from_date,
            to_date,
        ),
        email,
    )


@app.post("/calendar")
async def calendar_create(payload: CalendarCreate, account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)

    # Check if payload has attendees and use invite_event if so
    data = payload.model_dump()
    attendees = data.get("attendees", [])
    required_attendees = data.get("required_attendees", [])
    optional_attendees = data.get("optional_attendees", [])

    if attendees or required_attendees or optional_attendees:
        # Use invite_event to create and send meeting invites
        return await safe_call_with_retry(
            calendar.invite_event, email, acct, data
        )
    else:
        # Simple event without attendees
        return await safe_call_with_retry(
            calendar.create_event, email, acct, data
        )


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
        # Remove time/date keywords and use what's left
        cleaned = re.sub(
            r"\b(at|on|um|am|für|for|next|nächste[rn]?|today|tomorrow|morgen|"
            r"monday|tuesday|wednesday|thursday|friday|saturday|sunday|"
            r"montag|dienstag|mittwoch|donnerstag|freitag|samstag|sonntag|"
            r"\d{1,2}:\d{2}|\d{1,2}(am|pm|uhr)?)\b",
            "",
            remaining,
            flags=re.IGNORECASE,
        )
        cleaned = re.sub(r"\s+", " ", cleaned).strip()
        subject = cleaned if cleaned else "Event"

    # Build calendar create payload
    result = {
        "subject": subject,
        "start": parsed.start.isoformat(),
        "end": parsed.end.isoformat(),
    }

    if payload.location:
        result["location"] = payload.location

    if attendees:
        result["attendees"] = attendees

    return result


@app.delete("/calendar/{item_id}")
async def calendar_delete(
    item_id: str, changekey: Optional[str] = None, account: Optional[str] = None
):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(calendar.delete_event, email, acct, item_id)


class CalendarCancel(BaseModel):
    """Request model for cancelling a meeting."""

    message: Optional[str] = None


@app.post("/calendar/{item_id}/cancel")
async def calendar_cancel(
    item_id: str, payload: CalendarCancel, account: Optional[str] = None
):
    """Cancel a calendar event and notify all attendees."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        calendar.cancel_event, email, acct, item_id, payload.message
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        calendar.search_events, email, acct, q, days, from_date, to_date, limit
    )


@app.post("/calendar/invite")
async def calendar_invite(payload: CalendarInvite, account: Optional[str] = None):
    """Create a calendar event and send meeting invites to attendees."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        calendar.invite_event, email, acct, payload.model_dump()
    )


@app.get("/calendar/invites")
async def calendar_invites(
    limit: int = 50,
    account: Optional[str] = None,
):
    """List pending meeting invites from inbox."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(calendar.list_invites, email, acct, limit)


@app.post("/calendar/{item_id}/rsvp")
async def calendar_rsvp(
    item_id: str, payload: CalendarRsvp, account: Optional[str] = None
):
    """Respond to a meeting invite (accept/decline/tentative)."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        calendar.rsvp_event, email, acct, item_id, payload.response, payload.message
    )


@app.get("/mail")
async def mail_list(
    folder: str = "inbox",
    limit: int = 20,
    unread: bool = False,
    account: Optional[str] = None,
):
    email = current_account_email(account)
    acct = auth.get_account(email)
    key = cache_key("mail", account=acct, folder=folder, limit=limit, unread=unread)
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry, mail.list_messages, email, acct, folder, limit, unread
        ),
        email,
    )


@app.get("/mail/search")
async def mail_search(
    q: str,
    folder: str = "inbox",
    limit: int = 50,
    account: Optional[str] = None,
):
    """Search messages by subject, sender, or body content.

    Supports:
    - Simple text: "meeting notes"
    - Field-specific: "subject:meeting" or "from:john@example.com"
    - Boolean: "meeting AND notes"
    """
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.search_messages, email, acct, q, folder, limit
    )


@app.get("/mail/{item_id}")
async def mail_get(item_id: str, folder: str = "inbox", account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(mail.get_message, email, acct, item_id, folder)


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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.batch_get_messages, email, acct, payload.ids, payload.folder
    )


@app.post("/mail/send")
async def mail_send(payload: SendEmail, account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.send_message, email, acct, payload.model_dump()
    )


@app.post("/mail/fetch")
async def mail_fetch(payload: FetchMail, account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.fetch_messages,
        email,
        acct,
        payload.folder,
        payload.output,
        payload.format,
        payload.limit,
    )


# Draft endpoints (ews-test-ahe)


@app.post("/mail/draft")
async def draft_save(payload: DraftSave, account: Optional[str] = None):
    """Save a new draft to Exchange drafts folder."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.save_draft, email, acct, payload.model_dump()
    )


@app.put("/mail/draft/{item_id}")
async def draft_update(
    item_id: str, payload: DraftUpdate, account: Optional[str] = None
):
    """Update an existing draft."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    update_data = {k: v for k, v in payload.model_dump().items() if v is not None}
    return await safe_call_with_retry(
        mail.update_draft, email, acct, item_id, update_data
    )


@app.delete("/mail/draft/{item_id}")
async def draft_delete(item_id: str, account: Optional[str] = None):
    """Delete a draft."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(mail.delete_draft, email, acct, item_id)


@app.get("/mail/{item_id}/attachments")
async def mail_attachments_list(
    item_id: str, folder: str = "inbox", account: Optional[str] = None
):
    """List attachments for a message."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.list_attachments, email, acct, item_id, folder
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.download_attachment,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.delete_message, email, acct, item_id, folder, permanent
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.move_message,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(mail.empty_folder, email, acct, folder_name)


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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        mail.mark_as_spam,
        email,
        acct,
        item_id,
        payload.is_spam,
        payload.move,
    )


@app.get("/contacts")
async def contacts_list(
    limit: int = 100,
    search: Optional[str] = None,
    account: Optional[str] = None,
):
    email = current_account_email(account)
    acct = auth.get_account(email)
    key = cache_key("contacts", account=acct, limit=limit, search=search)
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry, contacts.list_contacts, email, acct, limit, search
        ),
        email,
    )


@app.get("/contacts/{item_id}")
async def contacts_get(item_id: str, account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(contacts.get_contact, email, acct, item_id)


@app.post("/contacts")
async def contacts_create(payload: ContactCreate, account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        contacts.create_contact, email, acct, payload.model_dump()
    )


@app.delete("/contacts/{item_id}")
async def contacts_delete(item_id: str, account: Optional[str] = None):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(contacts.delete_contact, email, acct, item_id)


@app.put("/contacts/{item_id}")
async def contacts_update(
    item_id: str, payload: ContactUpdate, account: Optional[str] = None
):
    """Update an existing contact."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    update_data = {k: v for k, v in payload.model_dump().items() if v is not None}
    return await safe_call_with_retry(
        contacts.update_contact, email, acct, item_id, update_data
    )


@app.get("/free")
async def free_slots(
    weeks: int = 1,
    duration: int = 30,
    limit: Optional[int] = None,
    account: Optional[str] = None,
):
    email = current_account_email(account)
    acct = auth.get_account(email)
    key = cache_key("free", account=acct, weeks=weeks, duration=duration, limit=limit)
    return await get_or_set(
        key,
        partial(
            safe_call_with_retry,
            free.find_free_slots,
            email,
            acct,
            weeks,
            duration,
            limit,
        ),
        email,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    try:
        target_email = resolve_person_alias(person)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    # Validate the target email resolves to a real mailbox (trx-bf2h)
    is_valid = await safe_call_with_retry(
        resolve.validate_email, email, acct, target_email
    )
    if not is_valid:
        raise HTTPException(
            status_code=404,
            detail=f"Email '{target_email}' does not resolve to a valid mailbox. "
            f"Use 'h8 addr resolve <query>' to search the directory.",
        )
    return await safe_call_with_retry(
        people.get_person_agenda,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    try:
        target_email = resolve_person_alias(person)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    # Validate the target email resolves to a real mailbox (trx-bf2h)
    is_valid = await safe_call_with_retry(
        resolve.validate_email, email, acct, target_email
    )
    if not is_valid:
        raise HTTPException(
            status_code=404,
            detail=f"Email '{target_email}' does not resolve to a valid mailbox. "
            f"Use 'h8 addr resolve <query>' to search the directory.",
        )
    return await safe_call_with_retry(
        people.get_person_free_slots,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
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
            resolve.validate_email, email, acct, target_email
        )
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
        people.find_common_free_slots,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    resource_list = [r.model_dump() for r in payload.resources]
    return await safe_call_with_retry(
        resources.resource_free,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    resource_list = [r.model_dump() for r in payload.resources]
    return await safe_call_with_retry(
        resources.resource_free_window,
        email,
        acct,
        resource_list,
        payload.from_date,
        payload.to_date,
    )


@app.post("/resource/agenda")
async def resource_agenda(
    payload: ResourceAgendaRequest, account: Optional[str] = None
):
    """Get bookings/events for each resource in a group."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    resource_list = [r.model_dump() for r in payload.resources]
    return await safe_call_with_retry(
        resources.resource_agenda,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        resolve.resolve_names, email, acct, q
    )


@app.get("/addr/validate")
async def addr_validate(
    email_addr: str,
    account: Optional[str] = None,
):
    """Validate whether an email address resolves to a real mailbox.

    Returns {"valid": true/false, "email": "..."}.
    """
    email = current_account_email(account)
    acct = auth.get_account(email)
    is_valid = await safe_call_with_retry(
        resolve.validate_email, email, acct, email_addr
    )
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(rules_oof.list_rules, email, acct)


@app.get("/rules/{rule_id}")
async def rules_get(rule_id: str, account: Optional[str] = None):
    """Get a specific rule by ID."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    result = await safe_call_with_retry(rules_oof.get_rule, email, acct, rule_id)
    if result is None:
        raise HTTPException(status_code=404, detail=f"Rule '{rule_id}' not found")
    return result


@app.post("/rules")
async def rules_create(payload: RuleCreate, account: Optional[str] = None):
    """Create a new inbox rule."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        rules_oof.create_rule,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        rules_oof.update_rule,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(rules_oof.enable_rule, email, acct, rule_id)


@app.post("/rules/{rule_id}/disable")
async def rules_disable(rule_id: str, account: Optional[str] = None):
    """Disable an inbox rule."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(rules_oof.disable_rule, email, acct, rule_id)


@app.delete("/rules/{rule_id}")
async def rules_delete(rule_id: str, account: Optional[str] = None):
    """Delete an inbox rule."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    await safe_call_with_retry(rules_oof.delete_rule, email, acct, rule_id)
    return {"success": True, "id": rule_id}


@app.get("/oof")
async def oof_get(account: Optional[str] = None):
    """Get Out-of-Office settings."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(rules_oof.get_oof_settings, email, acct)


@app.put("/oof")
async def oof_set(payload: OofSettings, account: Optional[str] = None):
    """Set Out-of-Office settings."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        rules_oof.set_oof_settings,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        rules_oof.enable_oof,
        email,
        acct,
        payload.internal_reply,
        payload.external_reply,
        payload.external_audience,
    )


@app.post("/oof/schedule")
async def oof_schedule(payload: OofSchedule, account: Optional[str] = None):
    """Schedule Out-of-Office for a future period."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        rules_oof.schedule_oof,
        email,
        acct,
        payload.start,
        payload.end,
        payload.internal_reply,
        payload.external_reply,
        payload.external_audience,
    )


@app.post("/oof/disable")
async def oof_disable(account: Optional[str] = None):
    """Disable Out-of-Office."""
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(rules_oof.disable_oof, email, acct)


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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        unsubscribe.scan_messages,
        email,
        acct,
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
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(
        unsubscribe.execute_unsubscribe,
        email,
        acct,
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
