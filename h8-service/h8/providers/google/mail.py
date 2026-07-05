"""Gmail implementation of the :class:`~h8.providers.base.MailBackend` contract.

``GoogleMailMixin`` is composed into
:class:`h8.providers.google.GoogleBackend`. It reaches the Gmail REST API through
``self._client.gmail()`` (a lazily-built ``googleapiclient`` service) and reads
account context from ``self.account``.

Response shapes are FROZEN to the EWS-derived JSON produced by ``h8/mail.py`` --
same keys, same nesting. Gmail has no per-item change token, so ``changekey`` is
always ``None``. Datetimes use the message ``internalDate`` (epoch ms) rendered as
ISO-8601 in UTC.

Design decisions (documented for gcal/gcontacts siblings and future readers):

* **Folder -> label mapping** (``_system_label`` / ``_folder_filter``):
  ``inbox->INBOX``, ``sent->SENT``, ``drafts->DRAFT``, ``trash->TRASH``,
  ``junk``/``spam``->``SPAM``. ``archive`` is not a Gmail label; it is expressed
  as the query ``-in:inbox -in:trash -in:spam``. Any other name is treated as a
  user label: resolved by name for reads (error if absent) and created on demand
  for writes.
* **Search translation** (``_translate_query``): ``|`` / `` OR `` split terms into
  a Gmail ``OR`` group; ``from:`` and ``subject:`` map to the native Gmail
  operators; ``body:`` and bare terms become bare Gmail terms (Gmail full-text
  already covers subject/sender/body). ``from_date``/``to_date`` become
  ``after:``/``before:`` with ``yyyy/mm/dd``.
* **Message bodies** (``_extract_body``): prefer the ``text/plain`` part; fall back
  to a tag-stripped ``text/html`` part. ``body_type`` is ``"text"`` when a plain
  part was used and ``"html"`` when only HTML was available.

Module-level helpers are prefixed with ``_`` and kept at module scope so the
sibling Google mixins cannot collide with them.
"""

from __future__ import annotations

import base64
import mailbox
import mimetypes
import os
import re
from collections import namedtuple
from datetime import datetime, timedelta, timezone
from email.message import EmailMessage
from email.utils import getaddresses, parseaddr
from typing import Any, List, Optional, Tuple

from h8.auth import AuthLoginRequired
from h8.oauth import LoginRequired
from h8.providers.base import (
    CAP_MAIL_SCHEDULED_SEND,
    BackendAuthError,
    BackendBusyError,
    BackendError,
    BackendNotSupported,
)

# Reused from the (backend-agnostic) unsubscribe module. Only the link-extraction
# and HTTP-visiting helpers are used here; none of them touch exchangelib.
from h8.unsubscribe import (
    UnsubscribeResult,
    _is_safe_domain,
    _is_safe_sender,
    _visit_unsubscribe_link,
    extract_unsubscribe_links,
)

USER_ID = "me"

# Folder name -> Gmail system label id.
_SYSTEM_LABELS = {
    "inbox": "INBOX",
    "sent": "SENT",
    "drafts": "DRAFT",
    "draft": "DRAFT",
    "trash": "TRASH",
    "junk": "SPAM",
    "spam": "SPAM",
    "starred": "STARRED",
    "important": "IMPORTANT",
    "unread": "UNREAD",
}

# ``archive`` is not a label -- it is "everything not in inbox/trash/spam".
_ARCHIVE_QUERY = "-in:inbox -in:trash -in:spam"

# Headers requested for list/metadata fetches.
_METADATA_HEADERS = ["Subject", "From", "To", "Cc", "Date", "Message-ID"]

# Lightweight stand-in for an exchangelib ``MessageHeader`` so the reused
# ``extract_unsubscribe_links`` helper (which reads ``.name``/``.value``) works.
_Header = namedtuple("_Header", ["name", "value"])

_TAG_RE = re.compile(r"<[^>]+>")
_STYLE_RE = re.compile(r"<(script|style)[^>]*>.*?</\1>", re.IGNORECASE | re.DOTALL)


# ---------------------------------------------------------------------------
# base64url helpers
# ---------------------------------------------------------------------------


def _b64url_decode(data: str) -> bytes:
    """Decode a Gmail base64url payload (tolerating missing padding)."""
    if isinstance(data, str):
        data = data.encode("ascii")
    data = data.replace(b"-", b"+").replace(b"_", b"/")
    padding = b"=" * (-len(data) % 4)
    return base64.b64decode(data + padding)


def _b64url_encode(raw: bytes) -> str:
    """Encode raw RFC 2822 bytes as an unpadded base64url string for Gmail."""
    return base64.urlsafe_b64encode(raw).decode("ascii")


# ---------------------------------------------------------------------------
# HTML / body helpers
# ---------------------------------------------------------------------------


def _strip_html(html: str) -> str:
    """Reduce an HTML fragment to readable plain text."""
    text = _STYLE_RE.sub(" ", html)
    text = re.sub(r"<br\s*/?>", "\n", text, flags=re.IGNORECASE)
    text = re.sub(r"</(p|div|tr|li|h[1-6])>", "\n", text, flags=re.IGNORECASE)
    text = _TAG_RE.sub("", text)
    # Collapse common HTML entities.
    for entity, char in (
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", '"'),
        ("&#39;", "'"),
    ):
        text = text.replace(entity, char)
    text = re.sub(r"[ \t]+\n", "\n", text)
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text.strip()


def _part_text(part: dict) -> str:
    """Decode a single MIME part's textual body."""
    body = part.get("body", {}) or {}
    data = body.get("data")
    if not data:
        return ""
    try:
        return _b64url_decode(data).decode("utf-8", errors="replace")
    except Exception:  # noqa: BLE001
        return ""


def _iter_parts(payload: dict):
    """Depth-first walk over a payload and all nested parts."""
    yield payload
    for part in payload.get("parts", []) or []:
        yield from _iter_parts(part)


def _extract_body(payload: dict) -> Tuple[str, str]:
    """Extract a text body from a full-format Gmail payload.

    Returns ``(body, body_type)`` where ``body_type`` is ``"text"`` when a
    ``text/plain`` part supplied the text and ``"html"`` when only HTML was
    available (and thus tag-stripped).
    """
    plain: Optional[str] = None
    html: Optional[str] = None
    for part in _iter_parts(payload):
        mime = (part.get("mimeType") or "").lower()
        # Skip attachment parts.
        if part.get("filename"):
            continue
        if mime == "text/plain" and plain is None:
            text = _part_text(part)
            if text:
                plain = text
        elif mime == "text/html" and html is None:
            text = _part_text(part)
            if text:
                html = text
    if plain is not None:
        return plain, "text"
    if html is not None:
        return _strip_html(html), "html"
    return "", "text"


# ---------------------------------------------------------------------------
# header / address helpers
# ---------------------------------------------------------------------------


def _headers_map(payload: dict) -> dict:
    """Case-insensitive ``{header_name: value}`` view of a payload's headers."""
    result: dict = {}
    for header in payload.get("headers", []) or []:
        name = (header.get("name") or "").lower()
        if name and name not in result:
            result[name] = header.get("value") or ""
    return result


def _single_address(value: Optional[str]) -> Optional[str]:
    """Return the bare email address from a ``From``-style header value."""
    if not value:
        return None
    _name, addr = parseaddr(value)
    return addr or None


def _address_list(value: Optional[str]) -> List[str]:
    """Return the list of bare email addresses from a ``To``/``Cc`` header."""
    if not value:
        return []
    return [addr for _name, addr in getaddresses([value]) if addr]


def _internaldate_to_iso(internal_date: Optional[str]) -> Optional[str]:
    """Convert a Gmail ``internalDate`` (epoch ms string) to ISO-8601 UTC."""
    if internal_date is None:
        return None
    try:
        seconds = int(internal_date) / 1000.0
    except (TypeError, ValueError):
        return None
    return datetime.fromtimestamp(seconds, tz=timezone.utc).isoformat()


# ---------------------------------------------------------------------------
# attachment helpers
# ---------------------------------------------------------------------------


def _attachment_parts(payload: dict) -> List[dict]:
    """Collect all parts that represent a downloadable attachment."""
    parts: List[dict] = []
    for part in _iter_parts(payload):
        filename = part.get("filename")
        body = part.get("body", {}) or {}
        if filename and (body.get("attachmentId") or body.get("data")):
            parts.append(part)
    return parts


def _has_attachments(payload: dict) -> bool:
    """Whether a payload carries at least one attachment part."""
    return bool(_attachment_parts(payload))


def _guess_mime(name: str) -> Tuple[str, str]:
    """Guess ``(maintype, subtype)`` for an attachment filename."""
    ctype, _enc = mimetypes.guess_type(name)
    if not ctype or "/" not in ctype:
        return "application", "octet-stream"
    maintype, subtype = ctype.split("/", 1)
    return maintype, subtype


def _load_attachment(spec: dict) -> Tuple[str, bytes]:
    """Resolve an attachment spec (``path`` or ``content``) to ``(name, bytes)``.

    Mirrors ``h8.mail.build_file_attachments``: ``content`` may be raw ``bytes``
    or a base64-encoded ``str``; ``path`` reads a local file.
    """
    content = spec.get("content")
    path = spec.get("path")
    if content is None and path:
        with open(path, "rb") as handle:
            content = handle.read()
    elif isinstance(content, str):
        content = base64.b64decode(content)
    if content is None:
        content = b""
    name = spec.get("name") or (os.path.basename(path) if path else "attachment")
    return name, content


# ---------------------------------------------------------------------------
# query translation
# ---------------------------------------------------------------------------


def _date_to_gmail(value: str, inclusive_end: bool = False) -> Optional[str]:
    """Convert an ISO date (or full ISO datetime) to Gmail ``yyyy/mm/dd``.

    Gmail's ``before:`` operator is exclusive while the h8 ``to_date`` bound is
    inclusive. Pass ``inclusive_end=True`` when formatting a ``before:`` bound so
    one day is added and the end date is included.
    """
    try:
        dt = datetime.fromisoformat(value)
    except ValueError:
        try:
            dt = datetime.strptime(value, "%Y-%m-%d")
        except ValueError:
            return None
    if inclusive_end:
        dt = dt + timedelta(days=1)
    return dt.strftime("%Y/%m/%d")


def _translate_term(term: str) -> str:
    """Translate a single h8 search term to a Gmail query fragment."""
    match = re.match(r"^(from|subject|body):(.+)$", term, re.IGNORECASE)
    if match:
        field = match.group(1).lower()
        value = match.group(2).strip()
        quoted = f'"{value}"' if " " in value else value
        if field == "from":
            return f"from:{quoted}"
        if field == "subject":
            return f"subject:{quoted}"
        # body: -> bare term (Gmail full-text search covers the body)
        return quoted
    return f'"{term}"' if " " in term else term


def _translate_query(
    query: str,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
) -> str:
    """Translate the h8 search syntax into a Gmail ``q=`` string.

    ``|`` or `` OR `` separate terms into a Gmail ``OR`` group; ``from:`` and
    ``subject:`` map to native operators; ``body:``/bare terms become bare terms.
    ``from_date``/``to_date`` are ANDed on as ``after:``/``before:``.
    """
    terms = [t.strip() for t in re.split(r"\s*\|\s*|\s+OR\s+", query or "") if t.strip()]
    fragments = [_translate_term(t) for t in terms]

    if len(fragments) > 1:
        group = "(" + " OR ".join(fragments) + ")"
    elif fragments:
        group = fragments[0]
    else:
        group = ""

    parts = [group] if group else []
    if from_date:
        gm = _date_to_gmail(from_date)
        if gm:
            parts.append(f"after:{gm}")
    if to_date:
        gm = _date_to_gmail(to_date, inclusive_end=True)
        if gm:
            parts.append(f"before:{gm}")
    return " ".join(parts)


# ---------------------------------------------------------------------------
# MIME construction
# ---------------------------------------------------------------------------


def _build_mime(
    to: Optional[List[str]] = None,
    cc: Optional[List[str]] = None,
    bcc: Optional[List[str]] = None,
    subject: str = "",
    body: str = "",
    html: bool = False,
    attachments: Optional[List[dict]] = None,
    headers: Optional[dict] = None,
) -> EmailMessage:
    """Build an :class:`email.message.EmailMessage` from h8 message fields."""
    msg = EmailMessage()
    if to:
        msg["To"] = ", ".join(to)
    if cc:
        msg["Cc"] = ", ".join(cc)
    if bcc:
        msg["Bcc"] = ", ".join(bcc)
    msg["Subject"] = subject or ""
    for name, value in (headers or {}).items():
        if value:
            msg[name] = value

    if html:
        msg.set_content(_strip_html(body or ""))
        msg.add_alternative(body or "", subtype="html")
    else:
        msg.set_content(body or "")

    for spec in attachments or []:
        name, content = _load_attachment(spec)
        maintype, subtype = _guess_mime(name)
        msg.add_attachment(content, maintype=maintype, subtype=subtype, filename=name)
    return msg


def _mime_to_gmail_body(msg: EmailMessage, thread_id: Optional[str] = None) -> dict:
    """Wrap an ``EmailMessage`` as a Gmail ``message`` request body."""
    body: dict = {"raw": _b64url_encode(msg.as_bytes())}
    if thread_id:
        body["threadId"] = thread_id
    return body


# ---------------------------------------------------------------------------
# message -> h8 dict
# ---------------------------------------------------------------------------


def _message_to_dict(msg: dict, include_body: bool = False) -> dict:
    """Convert a Gmail message resource to the frozen EWS-shaped dict."""
    payload = msg.get("payload", {}) or {}
    headers = _headers_map(payload)
    label_ids = msg.get("labelIds", []) or []

    result = {
        "id": msg.get("id"),
        "changekey": None,
        "subject": headers.get("subject"),
        "from": _single_address(headers.get("from")),
        "to": _address_list(headers.get("to")),
        "cc": _address_list(headers.get("cc")),
        "datetime_received": _internaldate_to_iso(msg.get("internalDate")),
        "is_read": "UNREAD" not in label_ids,
        "has_attachments": _has_attachments(payload),
    }
    if include_body:
        body, body_type = _extract_body(payload)
        result["body"] = body
        result["body_type"] = body_type
    return result


# ---------------------------------------------------------------------------
# execute wrapper (error translation)
# ---------------------------------------------------------------------------


def _execute(request: Any) -> Any:
    """Run ``request.execute()``, translating Google errors to backend errors."""
    try:
        return request.execute()
    except LoginRequired as exc:
        raise AuthLoginRequired(str(exc)) from exc
    except BackendError:
        raise
    except Exception as exc:  # noqa: BLE001
        status = getattr(getattr(exc, "resp", None), "status", None)
        try:
            status = int(status)
        except (TypeError, ValueError):
            status = None
        if status in (401, 403):
            raise BackendAuthError(str(exc)) from exc
        if status in (429, 500, 503):
            raise BackendBusyError(str(exc)) from exc
        raise BackendError(str(exc)) from exc


# ---------------------------------------------------------------------------
# mixin
# ---------------------------------------------------------------------------


class GoogleMailMixin:
    """Gmail-backed implementation of the mail domain methods."""

    # -- service access -----------------------------------------------------

    def _gmail(self) -> Any:
        """Return the cached Gmail service, mapping login errors to auth errors."""
        try:
            return self._client.gmail()
        except LoginRequired as exc:
            raise AuthLoginRequired(str(exc)) from exc

    # -- label / folder resolution -----------------------------------------

    def _resolve_label_id(
        self, svc: Any, folder: str, create: bool = False
    ) -> str:
        """Resolve a folder name to a Gmail label id.

        System folders map directly. User labels are looked up by name; when
        ``create`` is set a missing label is created, otherwise a ``ValueError``
        is raised (mirrors ``h8.mail.get_folder``).
        """
        name = (folder or "").strip()
        system = _SYSTEM_LABELS.get(name.lower())
        if system:
            return system
        if name.lower() == "archive":
            raise ValueError("'archive' is not a Gmail label")

        listing = _execute(svc.users().labels().list(userId=USER_ID))
        for label in listing.get("labels", []) or []:
            if (label.get("name") or "").lower() == name.lower():
                return label["id"]

        if create:
            created = _execute(
                svc.users()
                .labels()
                .create(
                    userId=USER_ID,
                    body={
                        "name": name,
                        "labelListVisibility": "labelShow",
                        "messageListVisibility": "show",
                    },
                )
            )
            return created["id"]
        raise ValueError(f"Folder '{folder}' not found")

    def _folder_filter(
        self, svc: Any, folder: str, unread: bool = False
    ) -> Tuple[List[str], str]:
        """Return ``(label_ids, extra_query)`` restricting a listing to a folder."""
        label_ids: List[str] = []
        extra_q = ""
        if (folder or "").lower() == "archive":
            extra_q = _ARCHIVE_QUERY
        else:
            label_ids.append(self._resolve_label_id(svc, folder, create=False))
        if unread:
            label_ids.append("UNREAD")
        return label_ids, extra_q

    def _list_message_ids(
        self,
        svc: Any,
        label_ids: List[str],
        query: str,
        limit: Optional[int],
    ) -> List[str]:
        """List message ids for a folder/query, honouring ``limit`` (paginated)."""
        ids: List[str] = []
        page_token: Optional[str] = None
        while True:
            remaining = None if limit is None else max(limit - len(ids), 0)
            if remaining == 0:
                break
            page_size = 500 if remaining is None else min(500, remaining)
            request = svc.users().messages().list(
                userId=USER_ID,
                labelIds=label_ids or None,
                q=query or None,
                maxResults=page_size,
                pageToken=page_token,
            )
            response = _execute(request)
            for entry in response.get("messages", []) or []:
                ids.append(entry["id"])
                if limit is not None and len(ids) >= limit:
                    return ids
            page_token = response.get("nextPageToken")
            if not page_token:
                break
        return ids

    def _get_message_resource(
        self, svc: Any, message_id: str, fmt: str = "metadata"
    ) -> dict:
        """Fetch one Gmail message resource in the requested format."""
        kwargs: dict = {"userId": USER_ID, "id": message_id, "format": fmt}
        if fmt == "metadata":
            kwargs["metadataHeaders"] = _METADATA_HEADERS
        return _execute(svc.users().messages().get(**kwargs))

    def _fetch_dicts(
        self, svc: Any, ids: List[str], include_body: bool = False
    ) -> List[dict]:
        """Fetch and convert several messages, skipping ones that error out."""
        fmt = "full" if include_body else "metadata"
        results: List[dict] = []
        for message_id in ids:
            try:
                msg = self._get_message_resource(svc, message_id, fmt)
            except BackendError:
                continue
            results.append(_message_to_dict(msg, include_body=include_body))
        return results

    # -- MailBackend: reads -------------------------------------------------

    def list_messages(
        self, folder: str = "inbox", limit: int = 20, unread: bool = False
    ) -> List[dict]:
        """List messages in ``folder`` (newest first)."""
        svc = self._gmail()
        label_ids, extra_q = self._folder_filter(svc, folder, unread)
        ids = self._list_message_ids(svc, label_ids, extra_q, limit)
        return self._fetch_dicts(svc, ids, include_body=False)

    def search_messages(
        self,
        query: str,
        folder: str = "inbox",
        limit: int = 50,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> List[dict]:
        """Search messages by subject/sender/body using the h8 query syntax."""
        svc = self._gmail()
        label_ids, folder_q = self._folder_filter(svc, folder, unread=False)
        translated = _translate_query(query, from_date, to_date)
        combined = " ".join(p for p in (translated, folder_q) if p)
        ids = self._list_message_ids(svc, label_ids, combined, limit)
        return self._fetch_dicts(svc, ids, include_body=False)

    def get_message(self, item_id: str, folder: str = "inbox") -> dict:
        """Fetch a single message including its text body."""
        svc = self._gmail()
        try:
            msg = self._get_message_resource(svc, item_id, "full")
        except BackendError as exc:
            return {"error": f"Failed to fetch message: {exc}"}
        return _message_to_dict(msg, include_body=True)

    def batch_get_messages(
        self, item_ids: List[str], folder: str = "inbox"
    ) -> List[dict]:
        """Fetch several messages by id (``None`` placeholders for failures)."""
        if not item_ids:
            return []
        svc = self._gmail()
        results: List[Optional[dict]] = []
        for message_id in item_ids:
            try:
                msg = self._get_message_resource(svc, message_id, "full")
            except BackendError:
                results.append(None)
                continue
            results.append(_message_to_dict(msg, include_body=True))
        return results

    def list_attachments(self, item_id: str, folder: str = "inbox") -> List[dict]:
        """List a message's attachments."""
        svc = self._gmail()
        try:
            msg = self._get_message_resource(svc, item_id, "full")
        except BackendError:
            return []
        parts = _attachment_parts(msg.get("payload", {}) or {})
        attachments = []
        for index, part in enumerate(parts):
            body = part.get("body", {}) or {}
            attachments.append(
                {
                    "index": index,
                    "name": part.get("filename") or f"attachment_{index}",
                    "size": body.get("size"),
                    "content_type": part.get("mimeType") or "application/octet-stream",
                }
            )
        return attachments

    def download_attachment(
        self,
        item_id: str,
        attachment_index: int,
        output_path: str,
        folder: str = "inbox",
    ) -> dict:
        """Download one attachment to disk."""
        svc = self._gmail()
        try:
            msg = self._get_message_resource(svc, item_id, "full")
        except BackendError as exc:
            return {"success": False, "error": f"Failed to download attachment: {exc}"}

        parts = _attachment_parts(msg.get("payload", {}) or {})
        if not parts:
            return {"success": False, "error": "Message has no attachments"}
        if attachment_index < 0 or attachment_index >= len(parts):
            return {
                "success": False,
                "error": f"Invalid attachment index: {attachment_index}",
            }

        part = parts[attachment_index]
        body = part.get("body", {}) or {}
        data = body.get("data")
        if not data and body.get("attachmentId"):
            fetched = _execute(
                svc.users()
                .messages()
                .attachments()
                .get(userId=USER_ID, messageId=item_id, id=body["attachmentId"])
            )
            data = fetched.get("data")
        content = _b64url_decode(data) if data else b""

        filename = part.get("filename") or f"attachment_{attachment_index}"
        if os.path.isdir(output_path):
            filepath = os.path.join(output_path, filename)
        else:
            filepath = output_path
        with open(filepath, "wb") as handle:
            handle.write(content)
        return {
            "success": True,
            "path": filepath,
            "name": filename,
            "size": len(content),
        }

    def fetch_messages(
        self,
        folder: str,
        output_dir: str,
        format: str = "maildir",
        limit: Optional[int] = None,
    ) -> dict:
        """Export messages to maildir or mbox."""
        svc = self._gmail()
        label_ids, extra_q = self._folder_filter(svc, folder, unread=False)
        ids = self._list_message_ids(svc, label_ids, extra_q, limit)

        if format == "maildir":
            return self._fetch_to_maildir(svc, ids, output_dir)
        if format == "mbox":
            return self._fetch_to_mbox(svc, ids, output_dir)
        return {"error": f"Unknown format: {format}"}

    def _raw_message(self, svc: Any, message_id: str) -> Tuple[bytes, dict]:
        """Return ``(raw_rfc822_bytes, message_resource)`` for a message id."""
        msg = self._get_message_resource(svc, message_id, "raw")
        raw = _b64url_decode(msg["raw"]) if msg.get("raw") else b""
        return raw, msg

    def _fetch_to_maildir(self, svc: Any, ids: List[str], output_dir: str) -> dict:
        cur_dir = os.path.join(output_dir, "cur")
        new_dir = os.path.join(output_dir, "new")
        tmp_dir = os.path.join(output_dir, "tmp")
        for path in (cur_dir, new_dir, tmp_dir):
            os.makedirs(path, exist_ok=True)

        count = 0
        for message_id in ids:
            try:
                raw, msg = self._raw_message(svc, message_id)
            except BackendError:
                continue
            label_ids = msg.get("labelIds", []) or []
            is_read = "UNREAD" not in label_ids
            iso = msg.get("internalDate")
            try:
                timestamp = int(iso) // 1000 if iso else int(datetime.now().timestamp())
            except (TypeError, ValueError):
                timestamp = int(datetime.now().timestamp())
            flags = "S" if is_read else ""
            filename = f"{timestamp}.{message_id[:20]}.h8:2,{flags}"
            target_dir = cur_dir if is_read else new_dir
            with open(os.path.join(target_dir, filename), "wb") as handle:
                handle.write(raw)
            count += 1
        return {"success": True, "count": count, "output": output_dir}

    def _fetch_to_mbox(self, svc: Any, ids: List[str], output_dir: str) -> dict:
        os.makedirs(output_dir, exist_ok=True)
        mbox_path = os.path.join(output_dir, "mail.mbox")
        box = mailbox.mbox(mbox_path)
        box.lock()
        count = 0
        try:
            for message_id in ids:
                try:
                    raw, _msg = self._raw_message(svc, message_id)
                except BackendError:
                    continue
                box.add(mailbox.mboxMessage(raw))
                count += 1
        finally:
            box.unlock()
            box.close()
        return {"success": True, "count": count, "output": mbox_path}

    # -- MailBackend: send / drafts ----------------------------------------

    def send_message(self, message_data: dict) -> dict:
        """Send a message via Gmail ``users.messages.send``."""
        if message_data.get("schedule_at"):
            raise BackendNotSupported(
                "scheduled send is not supported by the Gmail backend",
                capability=CAP_MAIL_SCHEDULED_SEND,
            )
        svc = self._gmail()

        to = message_data.get("to", []) or []
        cc = message_data.get("cc", []) or []
        bcc = message_data.get("bcc", []) or []
        subject = message_data.get("subject", "")
        body = message_data.get("body", "")
        html = bool(message_data.get("html", False))
        attachments = message_data.get("attachments") or []

        headers: dict = {}
        thread_id = message_data.get("thread_id")

        # Reply / forward: derive threading + subject prefix from an original id.
        reply_id = message_data.get("reply_to_id") or message_data.get("in_reply_to_id")
        forward_id = message_data.get("forward_of_id")
        original_id = reply_id or forward_id
        if original_id:
            original = self._get_message_resource(svc, original_id, "full")
            omap = _headers_map(original.get("payload", {}) or {})
            original_msg_id = omap.get("message-id")
            if original_msg_id:
                headers["In-Reply-To"] = original_msg_id
                references = omap.get("references")
                headers["References"] = (
                    f"{references} {original_msg_id}".strip()
                    if references
                    else original_msg_id
                )
            thread_id = thread_id or original.get("threadId")
            osubject = omap.get("subject", "")
            prefix = "Re: " if reply_id else "Fwd: "
            if not subject:
                subject = _prefixed_subject(prefix, osubject)
            if body:
                body = _quote_original(body, original, html)

        # Explicit threading headers (e.g. from a reply draft flow).
        if message_data.get("in_reply_to"):
            headers.setdefault("In-Reply-To", message_data["in_reply_to"])
        if message_data.get("references"):
            headers.setdefault("References", message_data["references"])

        msg = _build_mime(
            to=to,
            cc=cc,
            bcc=bcc,
            subject=subject,
            body=body,
            html=html,
            attachments=attachments,
            headers=headers,
        )
        request_body = _mime_to_gmail_body(msg, thread_id)
        _execute(svc.users().messages().send(userId=USER_ID, body=request_body))
        return {
            "success": True,
            "subject": subject,
            "to": to,
            "attachments": [_load_attachment(a)[0] for a in attachments],
        }

    def save_draft(self, draft_data: dict) -> dict:
        """Create a draft via Gmail ``users.drafts.create``."""
        svc = self._gmail()
        subject = draft_data.get("subject", "")
        headers: dict = {}
        if draft_data.get("in_reply_to"):
            headers["In-Reply-To"] = draft_data["in_reply_to"]
        if draft_data.get("references"):
            headers["References"] = draft_data["references"]

        msg = _build_mime(
            to=draft_data.get("to", []) or [],
            cc=draft_data.get("cc", []) or [],
            bcc=draft_data.get("bcc", []) or [],
            subject=subject,
            body=draft_data.get("body", ""),
            html=bool(draft_data.get("html", False)),
            headers=headers,
        )
        created = _execute(
            svc.users()
            .drafts()
            .create(userId=USER_ID, body={"message": _mime_to_gmail_body(msg)})
        )
        return {
            "success": True,
            "id": created.get("id"),
            "changekey": None,
            "subject": subject,
        }

    def update_draft(self, item_id: str, update_data: dict) -> dict:
        """Update an existing draft (fields not supplied are preserved)."""
        svc = self._gmail()
        try:
            existing = _execute(
                svc.users().drafts().get(userId=USER_ID, id=item_id, format="full")
            )
        except BackendError:
            return {"success": False, "error": f"Draft not found: {item_id}"}

        payload = (existing.get("message", {}) or {}).get("payload", {}) or {}
        headers = _headers_map(payload)
        current_body, current_type = _extract_body(payload)

        to = update_data["to"] if "to" in update_data else _address_list(
            headers.get("to")
        )
        cc = update_data["cc"] if "cc" in update_data else _address_list(
            headers.get("cc")
        )
        bcc = update_data["bcc"] if "bcc" in update_data else _address_list(
            headers.get("bcc")
        )
        subject = (
            update_data["subject"]
            if "subject" in update_data
            else headers.get("subject", "")
        )
        if "body" in update_data:
            body = update_data["body"]
            html = bool(update_data.get("html", False))
        else:
            body = current_body
            html = current_type == "html"

        msg = _build_mime(
            to=to or [],
            cc=cc or [],
            bcc=bcc or [],
            subject=subject or "",
            body=body,
            html=html,
        )
        updated = _execute(
            svc.users()
            .drafts()
            .update(
                userId=USER_ID,
                id=item_id,
                body={"message": _mime_to_gmail_body(msg)},
            )
        )
        return {
            "success": True,
            "id": updated.get("id", item_id),
            "changekey": None,
            "subject": subject or "",
        }

    def delete_draft(self, item_id: str) -> dict:
        """Delete a draft via Gmail ``users.drafts.delete``."""
        svc = self._gmail()
        try:
            _execute(svc.users().drafts().delete(userId=USER_ID, id=item_id))
        except BackendError as exc:
            return {"success": False, "error": f"Draft not found: {item_id} ({exc})"}
        return {"success": True, "id": item_id}

    # -- MailBackend: mutations --------------------------------------------

    def delete_message(
        self, item_id: str, folder: str = "inbox", permanent: bool = False
    ) -> dict:
        """Delete a message (trash by default, permanent on request)."""
        svc = self._gmail()
        try:
            if permanent:
                _execute(svc.users().messages().delete(userId=USER_ID, id=item_id))
                return {"success": True, "id": item_id, "action": "deleted"}
            _execute(svc.users().messages().trash(userId=USER_ID, id=item_id))
            return {"success": True, "id": item_id, "action": "moved_to_trash"}
        except BackendError as exc:
            return {"success": False, "error": f"Failed to delete message: {exc}"}

    def move_message(
        self,
        item_id: str,
        target_folder: str,
        source_folder: str = "inbox",
        create_folder: bool = False,
    ) -> dict:
        """Move a message to another folder via label add/remove."""
        svc = self._gmail()

        if target_folder.lower() == "trash":
            _execute(svc.users().messages().trash(userId=USER_ID, id=item_id))
            return {
                "success": True,
                "id": item_id,
                "new_id": item_id,
                "target_folder": target_folder,
            }

        add_labels: List[str] = []
        remove_labels: List[str] = []

        if target_folder.lower() == "archive":
            remove_labels.append("INBOX")
        else:
            try:
                target_label = self._resolve_label_id(
                    svc, target_folder, create=create_folder
                )
            except ValueError:
                return {
                    "success": False,
                    "error": (
                        f"Target folder not found: {target_folder}. "
                        "Use --create to create it."
                    ),
                }
            add_labels.append(target_label)
            try:
                remove_labels.append(
                    self._resolve_label_id(svc, source_folder, create=False)
                )
            except ValueError:
                pass

        try:
            _execute(
                svc.users()
                .messages()
                .modify(
                    userId=USER_ID,
                    id=item_id,
                    body={
                        "addLabelIds": add_labels,
                        "removeLabelIds": remove_labels,
                    },
                )
            )
        except BackendError as exc:
            return {"success": False, "error": f"Failed to move message: {exc}"}
        return {
            "success": True,
            "id": item_id,
            "new_id": item_id,
            "target_folder": target_folder,
        }

    def empty_folder(self, folder_name: str = "trash") -> dict:
        """Permanently delete every message carrying a folder's label."""
        svc = self._gmail()
        try:
            label_ids, extra_q = self._folder_filter(svc, folder_name, unread=False)
        except ValueError as exc:
            return {"success": False, "error": str(exc)}

        ids = self._list_message_ids(svc, label_ids, extra_q, limit=None)
        if not ids:
            return {"success": True, "deleted_count": 0, "folder": folder_name}

        try:
            for chunk in _chunks(ids, 1000):
                _execute(
                    svc.users().messages().batchDelete(
                        userId=USER_ID, body={"ids": chunk}
                    )
                )
        except BackendError as exc:
            return {"success": False, "error": f"Failed to empty folder: {exc}"}
        return {"success": True, "deleted_count": len(ids), "folder": folder_name}

    def batch_move_messages(
        self,
        folder: str,
        target_folder: str,
        older_than_days: int,
        query: Optional[str] = None,
        limit: int = 500,
        create_folder: bool = True,
        dry_run: bool = False,
    ) -> dict:
        """Bulk-move messages older than ``older_than_days`` (optionally filtered)."""
        svc = self._gmail()
        label_ids, extra_q = self._folder_filter(svc, folder, unread=False)

        q_parts = [f"older_than:{older_than_days}d"]
        if extra_q:
            q_parts.append(extra_q)
        if query:
            q_parts.append(f'"{query}"' if " " in query else query)
        gmail_q = " ".join(q_parts)

        ids = self._list_message_ids(svc, label_ids, gmail_q, limit)
        matches = self._fetch_dicts(svc, ids, include_body=False)
        matched = [
            {
                "id": m["id"],
                "subject": m["subject"],
                "from": m["from"],
                "datetime_received": m["datetime_received"],
            }
            for m in matches
        ]

        moved = 0
        errors: List[str] = []
        if not dry_run and matched:
            add_labels: List[str] = []
            remove_labels: List[str] = []
            if target_folder.lower() == "archive":
                remove_labels.append("INBOX")
            else:
                try:
                    add_labels.append(
                        self._resolve_label_id(
                            svc, target_folder, create=create_folder
                        )
                    )
                except ValueError:
                    return {
                        "success": False,
                        "error": f"Target folder not found: {target_folder}",
                    }
            try:
                remove_labels.append(self._resolve_label_id(svc, folder, create=False))
            except ValueError:
                pass
            for entry in matched:
                try:
                    _execute(
                        svc.users()
                        .messages()
                        .modify(
                            userId=USER_ID,
                            id=entry["id"],
                            body={
                                "addLabelIds": add_labels,
                                "removeLabelIds": remove_labels,
                            },
                        )
                    )
                    moved += 1
                except BackendError as exc:
                    errors.append(f"{entry['id']}: {exc}")

        return {
            "success": True,
            "action": "move-old",
            "folder": folder,
            "target_folder": target_folder,
            "older_than_days": older_than_days,
            "query": query,
            "limit": limit,
            "dry_run": dry_run,
            "matched_count": len(matched),
            "moved_count": moved if not dry_run else 0,
            "errors": errors,
            "matches": matched,
        }

    def batch_mark_messages(
        self,
        folder: str,
        read: bool,
        ids: Optional[List[str]] = None,
        older_than_days: Optional[int] = None,
        query: Optional[str] = None,
        limit: int = 500,
        dry_run: bool = False,
    ) -> dict:
        """Bulk mark messages read/unread (by id list or folder/age/query)."""
        svc = self._gmail()

        if ids:
            target_ids = list(ids)
        else:
            label_ids, extra_q = self._folder_filter(svc, folder, unread=False)
            q_parts: List[str] = []
            if extra_q:
                q_parts.append(extra_q)
            if older_than_days is not None:
                q_parts.append(f"older_than:{older_than_days}d")
            if query:
                q_parts.append(f'"{query}"' if " " in query else query)
            target_ids = self._list_message_ids(
                svc, label_ids, " ".join(q_parts), limit
            )

        matches = self._fetch_dicts(svc, target_ids, include_body=False)
        matched = [
            {
                "id": m["id"],
                "subject": m["subject"],
                "from": m["from"],
                "datetime_received": m["datetime_received"],
                "is_read": m["is_read"],
            }
            for m in matches
        ]

        updated = 0
        errors: List[str] = []
        if not dry_run and matched:
            add_labels = [] if read else ["UNREAD"]
            remove_labels = ["UNREAD"] if read else []
            match_ids = [m["id"] for m in matched]
            try:
                for chunk in _chunks(match_ids, 1000):
                    _execute(
                        svc.users().messages().batchModify(
                            userId=USER_ID,
                            body={
                                "ids": chunk,
                                "addLabelIds": add_labels,
                                "removeLabelIds": remove_labels,
                            },
                        )
                    )
                updated = len(match_ids)
            except BackendError as exc:
                errors.append(str(exc))

        return {
            "success": True,
            "action": "mark",
            "folder": folder,
            "read": read,
            "older_than_days": older_than_days,
            "query": query,
            "limit": limit,
            "dry_run": dry_run,
            "matched_count": len(matched),
            "updated_count": updated if not dry_run else 0,
            "errors": errors,
            "matches": matched,
        }

    def mark_as_spam(
        self, item_id: str, is_spam: bool = True, move_to_junk: bool = True
    ) -> dict:
        """Mark a message as spam / not spam via the SPAM label."""
        svc = self._gmail()
        try:
            if is_spam:
                add_labels = ["SPAM"]
                remove_labels = ["INBOX"] if move_to_junk else []
                _execute(
                    svc.users().messages().modify(
                        userId=USER_ID,
                        id=item_id,
                        body={"addLabelIds": add_labels, "removeLabelIds": remove_labels},
                    )
                )
                if move_to_junk:
                    return {
                        "success": True,
                        "id": item_id,
                        "action": "marked_as_spam",
                        "moved_to": "junk",
                    }
                return {"success": True, "id": item_id, "action": "marked_as_spam"}

            add_labels = ["INBOX"] if move_to_junk else []
            _execute(
                svc.users().messages().modify(
                    userId=USER_ID,
                    id=item_id,
                    body={"addLabelIds": add_labels, "removeLabelIds": ["SPAM"]},
                )
            )
            if move_to_junk:
                return {
                    "success": True,
                    "id": item_id,
                    "action": "marked_as_not_spam",
                    "moved_to": "inbox",
                }
            return {"success": True, "id": item_id, "action": "marked_as_not_spam"}
        except BackendError as exc:
            return {"success": False, "error": f"Failed to mark as spam: {exc}"}

    # -- MailBackend: unsubscribe ------------------------------------------

    def _message_headers_and_body(
        self, msg: dict
    ) -> Tuple[List[_Header], str, str]:
        """Return ``(header_objs, body_text, body_type)`` for a full message."""
        payload = msg.get("payload", {}) or {}
        headers = [
            _Header(name=h.get("name", ""), value=h.get("value", ""))
            for h in payload.get("headers", []) or []
        ]
        body, body_type = _extract_body(payload)
        return headers, body, body_type

    def scan_unsubscribe(
        self,
        folder: str = "inbox",
        sender: Optional[str] = None,
        search: Optional[str] = None,
        limit: int = 50,
        safe_senders: Optional[List[str]] = None,
        blocked_patterns: Optional[List[str]] = None,
    ) -> List[dict]:
        """Scan messages for unsubscribe links (dry run) reusing ``unsubscribe.py``."""
        svc = self._gmail()
        safe_senders = safe_senders or []
        blocked_patterns = blocked_patterns or []

        label_ids, extra_q = self._folder_filter(svc, folder, unread=False)
        q_parts = [extra_q] if extra_q else []
        if search:
            q_parts.append(f'"{search}"' if " " in search else search)
        if sender:
            q_parts.append(f"from:{sender}")
        fetch_limit = limit
        ids = self._list_message_ids(svc, label_ids, " ".join(q_parts), fetch_limit)

        results: List[dict] = []
        for message_id in ids:
            try:
                msg = self._get_message_resource(svc, message_id, "full")
            except BackendError:
                continue
            headers, body, body_type = self._message_headers_and_body(msg)
            hmap = _headers_map(msg.get("payload", {}) or {})
            sender_email = _single_address(hmap.get("from")) or ""
            subject = hmap.get("subject") or "(no subject)"

            if _is_safe_sender(sender_email, safe_senders):
                results.append(
                    UnsubscribeResult(
                        message_id=message_id,
                        sender=sender_email,
                        subject=subject,
                        status="skipped",
                        error="safe sender",
                    ).to_dict()
                )
                continue

            links = extract_unsubscribe_links(headers, body, body_type)
            filtered = [
                link
                for link in links
                if link.is_mailto or _is_safe_domain(link.url, blocked_patterns)
            ]
            results.append(
                UnsubscribeResult(
                    message_id=message_id,
                    sender=sender_email,
                    subject=subject,
                    links=filtered,
                    status="found" if filtered else "no_link",
                ).to_dict()
            )
        return results

    def execute_unsubscribe(
        self,
        item_ids: List[str],
        safe_senders: Optional[List[str]] = None,
        blocked_patterns: Optional[List[str]] = None,
        trusted_domains: Optional[List[str]] = None,
        rate_limit_seconds: float = 2.0,
    ) -> List[dict]:
        """Visit unsubscribe links for the given messages (reuses ``unsubscribe.py``)."""
        import time

        svc = self._gmail()
        safe_senders = safe_senders or []
        blocked_patterns = blocked_patterns or []
        trusted_domains = trusted_domains or []

        results: List[dict] = []
        for i, message_id in enumerate(item_ids):
            if i > 0:
                time.sleep(rate_limit_seconds)
            try:
                msg = self._get_message_resource(svc, message_id, "full")
            except BackendError as exc:
                results.append(
                    UnsubscribeResult(
                        message_id=message_id,
                        sender="",
                        subject="",
                        status="failed",
                        error=f"message not found ({exc})",
                    ).to_dict()
                )
                continue

            headers, body, body_type = self._message_headers_and_body(msg)
            hmap = _headers_map(msg.get("payload", {}) or {})
            sender_email = _single_address(hmap.get("from")) or ""
            subject = hmap.get("subject") or "(no subject)"

            if _is_safe_sender(sender_email, safe_senders):
                results.append(
                    UnsubscribeResult(
                        message_id=message_id,
                        sender=sender_email,
                        subject=subject,
                        status="skipped",
                        error="safe sender",
                    ).to_dict()
                )
                continue

            links = extract_unsubscribe_links(headers, body, body_type)
            filtered = [
                link
                for link in links
                if link.is_mailto or _is_safe_domain(link.url, blocked_patterns)
            ]
            if not filtered:
                results.append(
                    UnsubscribeResult(
                        message_id=message_id,
                        sender=sender_email,
                        subject=subject,
                        status="no_link",
                    ).to_dict()
                )
                continue

            result = _visit_unsubscribe_link(
                message_id, sender_email, subject, filtered, trusted_domains
            )
            results.append(result.to_dict())
        return results


# ---------------------------------------------------------------------------
# small module-level helpers used by the mixin
# ---------------------------------------------------------------------------


def _chunks(items: List[Any], size: int):
    """Yield ``items`` in lists of at most ``size``."""
    for start in range(0, len(items), size):
        yield items[start : start + size]


def _prefixed_subject(prefix: str, subject: str) -> str:
    """Return ``subject`` with ``prefix`` (Re:/Fwd:) added if not already present."""
    subject = subject or ""
    lowered = subject.lower()
    if prefix.lower().strip().rstrip(":") in ("re",) and lowered.startswith("re:"):
        return subject
    if prefix.lower().strip().rstrip(":") in ("fwd", "fw") and (
        lowered.startswith("fwd:") or lowered.startswith("fw:")
    ):
        return subject
    return f"{prefix}{subject}"


def _quote_original(new_body: str, original: dict, html: bool) -> str:
    """Append a quoted copy of the original message body to ``new_body``."""
    payload = original.get("payload", {}) or {}
    hmap = _headers_map(payload)
    original_body, _btype = _extract_body(payload)
    sender = hmap.get("from", "")
    date = hmap.get("date", "")
    attribution = f"On {date}, {sender} wrote:" if (date or sender) else "Original message:"
    quoted = "\n".join(f"> {line}" for line in original_body.splitlines())
    return f"{new_body}\n\n{attribution}\n{quoted}"
