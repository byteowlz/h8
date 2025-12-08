"""
Background EWS service that uses the existing Python exchangelib logic and exposes
cached endpoints for the Rust CLI.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import time
from functools import partial
from typing import Any, Dict, Optional

import uvicorn
from fastapi import FastAPI
from fastapi import HTTPException
from fastapi.concurrency import run_in_threadpool
from pydantic import BaseModel, Field, field_validator

from h8 import auth, calendar, contacts, free, mail  # avoid circular import
from h8.config import get_config
from exchangelib.errors import UnauthorizedError

log = logging.getLogger(__name__)

DEFAULT_PORT = int(os.environ.get("H8_SERVICE_PORT", "8787"))
DEFAULT_HOST = os.environ.get("H8_SERVICE_HOST", "127.0.0.1")
REFRESH_INTERVAL = int(os.environ.get("H8_SERVICE_REFRESH_SECONDS", "300"))
CACHE_TTL = int(os.environ.get("H8_SERVICE_CACHE_TTL", "300"))
LOG_LEVEL = os.environ.get("H8_SERVICE_LOGLEVEL", "INFO").upper()


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


def current_account(requested: Optional[str]) -> str:
    config = get_config()
    return requested or config.get("account")


def current_ews_account(requested: Optional[str]):
    return auth.get_account(current_account(requested))


async def safe_call(func, *args, **kwargs):
    try:
        return await run_in_threadpool(func, *args, **kwargs)
    except UnauthorizedError as exc:
        raise HTTPException(status_code=401, detail=str(exc))
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))


app = FastAPI(title="h8-service", version="0.1.0")
cache: Dict[str, CacheEntry] = {}
cache_lock = asyncio.Lock()


def cache_key(prefix: str, **params: Any) -> str:
    parts = [prefix] + [f"{k}={v}" for k, v in sorted(params.items())]
    return "|".join(parts)


async def get_or_set(key: str, producer):
    async with cache_lock:
        entry = cache.get(key)
        if entry and entry.fresh():
            log.debug("cache hit: %s", key)
            return entry.data
    log.debug("cache miss: %s", key)
    try:
        data = await producer()
    except UnauthorizedError as exc:
        raise HTTPException(status_code=401, detail=str(exc))
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))
    async with cache_lock:
        cache[key] = CacheEntry(data=data, ts=time.time())
    return data


async def refresh_defaults():
    account = current_ews_account(None)
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
        )
    except Exception:
        log.exception("default refresh failed")


@app.on_event("startup")
async def startup() -> None:
    level = getattr(logging, LOG_LEVEL, logging.INFO)
    logging.basicConfig(level=level)
    app.state.refresh_task = asyncio.create_task(_refresh_loop())


@app.on_event("shutdown")
async def shutdown() -> None:
    task: asyncio.Task = app.state.refresh_task
    task.cancel()
    with contextlib.suppress(Exception):
        await task


async def _refresh_loop():
    while True:
        await refresh_defaults()
        await asyncio.sleep(REFRESH_INTERVAL)


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.get("/calendar")
async def calendar_list(
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
    account: Optional[str] = None,
):
    acct = current_ews_account(account)
    key = cache_key("calendar", account=acct, days=days, from_date=from_date, to_date=to_date)
    return await get_or_set(key, partial(safe_call, calendar.list_events, acct, days, from_date, to_date))


@app.post("/calendar")
async def calendar_create(payload: CalendarCreate, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(calendar.create_event, acct, payload.model_dump())


@app.delete("/calendar/{item_id}")
async def calendar_delete(item_id: str, changekey: Optional[str] = None, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(calendar.delete_event, acct, item_id)


@app.get("/mail")
async def mail_list(
    folder: str = "inbox",
    limit: int = 20,
    unread: bool = False,
    account: Optional[str] = None,
):
    acct = current_ews_account(account)
    key = cache_key("mail", account=acct, folder=folder, limit=limit, unread=unread)
    return await get_or_set(key, partial(safe_call, mail.list_messages, acct, folder, limit, unread))


@app.get("/mail/{item_id}")
async def mail_get(item_id: str, folder: str = "inbox", account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(mail.get_message, acct, item_id, folder)


@app.post("/mail/send")
async def mail_send(payload: SendEmail, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(mail.send_message, acct, payload.model_dump())


@app.post("/mail/fetch")
async def mail_fetch(payload: FetchMail, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(
        mail.fetch_messages,
        acct,
        payload.folder,
        payload.output,
        payload.format,
        payload.limit,
    )


@app.get("/contacts")
async def contacts_list(
    limit: int = 100,
    search: Optional[str] = None,
    account: Optional[str] = None,
):
    acct = current_ews_account(account)
    key = cache_key("contacts", account=acct, limit=limit, search=search)
    return await get_or_set(key, partial(safe_call, contacts.list_contacts, acct, limit, search))


@app.get("/contacts/{item_id}")
async def contacts_get(item_id: str, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(contacts.get_contact, acct, item_id)


@app.post("/contacts")
async def contacts_create(payload: ContactCreate, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(contacts.create_contact, acct, payload.model_dump())


@app.delete("/contacts/{item_id}")
async def contacts_delete(item_id: str, account: Optional[str] = None):
    acct = current_ews_account(account)
    return await safe_call(contacts.delete_contact, acct, item_id)


@app.get("/free")
async def free_slots(
    weeks: int = 1,
    duration: int = 30,
    limit: Optional[int] = None,
    account: Optional[str] = None,
):
    acct = current_ews_account(account)
    key = cache_key("free", account=acct, weeks=weeks, duration=duration, limit=limit)
    return await get_or_set(key, partial(safe_call, free.find_free_slots, acct, weeks, duration, limit))


def main() -> None:
    uvicorn.run(
        "h8.service:app",
        host=DEFAULT_HOST,
        port=DEFAULT_PORT,
        log_level="info",
    )


if __name__ == "__main__":
    main()
