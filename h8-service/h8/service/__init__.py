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

from h8 import auth, calendar, contacts, free, mail
from h8.config import get_config
from exchangelib.errors import UnauthorizedError

log = logging.getLogger(__name__)

DEFAULT_PORT = int(os.environ.get("H8_SERVICE_PORT", "8787"))
DEFAULT_HOST = os.environ.get("H8_SERVICE_HOST", "127.0.0.1")
REFRESH_INTERVAL = int(os.environ.get("H8_SERVICE_REFRESH_SECONDS", "300"))
CACHE_TTL = int(os.environ.get("H8_SERVICE_CACHE_TTL", "300"))
LOG_LEVEL = os.environ.get("H8_SERVICE_LOGLEVEL", "INFO").upper()
# Token refresh interval in seconds (default: 50 minutes)
TOKEN_REFRESH_INTERVAL = int(os.environ.get("H8_SERVICE_TOKEN_REFRESH_SECONDS", "3000"))


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


class SendEmail(BaseModel):
    to: list[str]
    cc: list[str] = Field(default_factory=list)
    subject: str
    body: str = ""
    html: bool = False

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
    Execute a function with automatic retry on UnauthorizedError.

    If an UnauthorizedError occurs, refreshes the token and retries once.
    """
    try:
        return await run_in_threadpool(func, *args, **kwargs)
    except UnauthorizedError as exc:
        log.warning(
            "UnauthorizedError for %s, refreshing token and retrying...", account_email
        )
        try:
            # Refresh the account token
            new_account = auth.refresh_account(account_email)
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
                "UnauthorizedError during cache fill for %s, refreshing...",
                account_email,
            )
            auth.refresh_account(account_email)
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
    """Periodically refresh authentication tokens before they expire."""
    try:
        email = current_account_email(None)
        log.info("Proactively refreshing token for %s", email)
        auth.refresh_account(email)
    except Exception as e:
        log.warning("Token refresh failed: %s", e)


@app.on_event("startup")
async def startup() -> None:
    level = getattr(logging, LOG_LEVEL, logging.INFO)
    logging.basicConfig(level=level)
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
    return await safe_call_with_retry(
        calendar.create_event, email, acct, payload.model_dump()
    )


@app.delete("/calendar/{item_id}")
async def calendar_delete(
    item_id: str, changekey: Optional[str] = None, account: Optional[str] = None
):
    email = current_account_email(account)
    acct = auth.get_account(email)
    return await safe_call_with_retry(calendar.delete_event, email, acct, item_id)


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


def main() -> None:
    uvicorn.run(
        "h8.service:app",
        host=DEFAULT_HOST,
        port=DEFAULT_PORT,
        log_level="info",
    )


if __name__ == "__main__":
    main()
