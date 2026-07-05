"""Unit tests for the Gmail backend mixin (``h8.providers.google.mail``).

The Gmail REST service is replaced by :class:`FakeGmailService`, an in-memory
stand-in for the ``users().messages()/drafts()/labels()`` call chains. No
``googleapiclient`` discovery/network code runs. Tests assert:

- h8 -> Gmail query translation (several cases)
- folder -> label mapping (system, archive, user-label create)
- list/get response-shape parity against golden key sets
- send MIME construction (raw decodes to expected headers + attachment)
- draft create/update/delete roundtrip
- batch mark / move label diffs
"""

from __future__ import annotations

import base64
import email
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest

from h8.providers.base import AccountConfig, BackendNotSupported
from h8.providers.google import mail as gmail
from h8.providers.google.mail import (
    GoogleMailMixin,
    _date_to_gmail,
    _extract_body,
    _internaldate_to_iso,
    _translate_query,
)

LIST_KEYS = {
    "id",
    "changekey",
    "subject",
    "from",
    "to",
    "cc",
    "datetime_received",
    "is_read",
    "has_attachments",
}
GET_KEYS = LIST_KEYS | {"body", "body_type"}


# ---------------------------------------------------------------------------
# fake Gmail service
# ---------------------------------------------------------------------------


class FakeHttpError(Exception):
    """Minimal stand-in for ``googleapiclient.errors.HttpError``."""

    def __init__(self, status: int) -> None:
        super().__init__(f"HTTP {status}")
        self.resp = SimpleNamespace(status=status)


class _Req:
    """A request whose ``execute()`` runs a stored callable."""

    def __init__(self, fn):
        self._fn = fn

    def execute(self):
        return self._fn()


class _Messages:
    def __init__(self, svc):
        self.svc = svc

    def list(self, userId, labelIds=None, q=None, maxResults=None, pageToken=None):
        self.svc.calls.append(("messages.list", dict(labelIds=labelIds, q=q,
                                                     maxResults=maxResults)))
        return _Req(lambda: self.svc._list_impl(labelIds, maxResults))

    def get(self, userId, id, format="metadata", metadataHeaders=None):
        self.svc.calls.append(("messages.get", dict(id=id, format=format)))
        return _Req(lambda: self.svc._get_impl(id))

    def send(self, userId, body):
        self.svc.calls.append(("messages.send", dict(body=body)))
        return _Req(lambda: {"id": "sent_1", "threadId": body.get("threadId", "t_x")})

    def trash(self, userId, id):
        self.svc.calls.append(("messages.trash", dict(id=id)))
        return _Req(lambda: {"id": id, "labelIds": ["TRASH"]})

    def delete(self, userId, id):
        self.svc.calls.append(("messages.delete", dict(id=id)))
        return _Req(lambda: None)

    def modify(self, userId, id, body):
        self.svc.calls.append(("messages.modify", dict(id=id, body=body)))
        return _Req(lambda: {"id": id})

    def batchModify(self, userId, body):
        self.svc.calls.append(("messages.batchModify", dict(body=body)))
        return _Req(lambda: None)

    def batchDelete(self, userId, body):
        self.svc.calls.append(("messages.batchDelete", dict(body=body)))
        return _Req(lambda: None)

    def attachments(self):
        return _Attachments(self.svc)


class _Attachments:
    def __init__(self, svc):
        self.svc = svc

    def get(self, userId, messageId, id):
        self.svc.calls.append(("attachments.get", dict(messageId=messageId, id=id)))
        data = self.svc.attachments[id]
        return _Req(lambda: {"data": base64.urlsafe_b64encode(data).decode(),
                             "size": len(data)})


class _Drafts:
    def __init__(self, svc):
        self.svc = svc

    def create(self, userId, body):
        self.svc.calls.append(("drafts.create", dict(body=body)))
        draft_id = f"draft_{len(self.svc.drafts) + 1}"
        self.svc.drafts[draft_id] = {"id": draft_id, "message": body["message"]}
        return _Req(lambda: {"id": draft_id,
                             "message": {"id": "m_1", "threadId": "t_1"}})

    def get(self, userId, id, format="full"):
        self.svc.calls.append(("drafts.get", dict(id=id)))

        def _run():
            if id not in self.svc.drafts:
                raise FakeHttpError(404)
            stored = self.svc.drafts[id]
            # Gmail returns a parsed payload; the fake stores only the raw MIME,
            # so parse it here to mimic ``format=full``.
            payload = _raw_to_payload(stored["message"]["raw"])
            return {"id": id, "message": {"labelIds": ["DRAFT"], "payload": payload}}

        return _Req(_run)

    def update(self, userId, id, body):
        self.svc.calls.append(("drafts.update", dict(id=id, body=body)))
        self.svc.drafts[id] = {"id": id, "message": body["message"]}
        return _Req(lambda: {"id": id, "message": {"id": "m_1"}})

    def delete(self, userId, id):
        self.svc.calls.append(("drafts.delete", dict(id=id)))

        def _run():
            if id not in self.svc.drafts:
                raise FakeHttpError(404)
            del self.svc.drafts[id]
            return None

        return _Req(_run)


class _Labels:
    def __init__(self, svc):
        self.svc = svc

    def list(self, userId):
        self.svc.calls.append(("labels.list", {}))
        return _Req(lambda: {"labels": list(self.svc.labels)})

    def create(self, userId, body):
        self.svc.calls.append(("labels.create", dict(body=body)))
        new_id = f"Label_{len(self.svc.labels) + 1}"
        label = {"id": new_id, "name": body["name"]}
        self.svc.labels.append(label)
        return _Req(lambda: label)


class _Users:
    def __init__(self, svc):
        self.svc = svc

    def messages(self):
        return _Messages(self.svc)

    def drafts(self):
        return _Drafts(self.svc)

    def labels(self):
        return _Labels(self.svc)


class FakeGmailService:
    """In-memory Gmail service usable in place of a ``googleapiclient`` build."""

    def __init__(self):
        self.messages = {}
        self.drafts = {}
        self.labels = []
        self.attachments = {}
        self.calls = []

    def users(self):
        return _Users(self)

    # -- impls -----------------------------------------------------------

    def _list_impl(self, label_ids, max_results):
        ids = []
        for mid, msg in self.messages.items():
            mlabels = msg.get("labelIds", [])
            if label_ids and not all(lbl in mlabels for lbl in label_ids):
                continue
            ids.append(mid)
        if max_results:
            ids = ids[:max_results]
        return {"messages": [{"id": i, "threadId": self.messages[i]["threadId"]}
                             for i in ids]}

    def _get_impl(self, message_id):
        if message_id not in self.messages:
            raise FakeHttpError(404)
        return self.messages[message_id]


def _b64(text: str) -> str:
    return base64.urlsafe_b64encode(text.encode()).decode()


def _raw_to_payload(raw_b64: str) -> dict:
    """Parse a base64url RFC 2822 message into a Gmail-style ``payload`` dict."""
    raw = base64.urlsafe_b64decode(raw_b64.encode())
    msg = email.message_from_bytes(raw)
    headers = [{"name": k, "value": v} for k, v in msg.items()]
    text = ""
    if msg.is_multipart():
        for part in msg.walk():
            if part.get_content_type() == "text/plain" and not part.get_filename():
                payload = part.get_payload(decode=True)
                text = payload.decode() if payload else ""
                break
    else:
        payload = msg.get_payload(decode=True)
        text = payload.decode() if payload else ""
    return {
        "mimeType": msg.get_content_type(),
        "headers": headers,
        "parts": [
            {"mimeType": "text/plain", "filename": "",
             "body": {"data": _b64(text)}}
        ],
    }


def make_message(
    mid,
    subject="Hello",
    frm="Alice <alice@example.com>",
    to="me@gmail.com",
    cc=None,
    labels=None,
    internal="1700000000000",
    plain=None,
    html=None,
    attachments=None,
    message_id_header=None,
    references=None,
):
    headers = [
        {"name": "Subject", "value": subject},
        {"name": "From", "value": frm},
        {"name": "To", "value": to},
        {"name": "Date", "value": "Wed, 15 Nov 2023 22:13:20 +0000"},
    ]
    if cc:
        headers.append({"name": "Cc", "value": cc})
    if message_id_header:
        headers.append({"name": "Message-ID", "value": message_id_header})
    if references:
        headers.append({"name": "References", "value": references})

    parts = []
    if plain is not None:
        parts.append({"mimeType": "text/plain", "filename": "",
                      "body": {"data": _b64(plain)}})
    if html is not None:
        parts.append({"mimeType": "text/html", "filename": "",
                      "body": {"data": _b64(html)}})
    if attachments:
        parts.extend(attachments)

    payload = {"mimeType": "multipart/mixed", "headers": headers, "parts": parts}
    return {
        "id": mid,
        "threadId": f"t_{mid}",
        "labelIds": labels or ["INBOX"],
        "internalDate": internal,
        "snippet": "snippet",
        "payload": payload,
    }


def make_backend(svc):
    class _Backend(GoogleMailMixin):
        def __init__(self, service):
            self._client = SimpleNamespace(gmail=lambda: service)
            self.account = AccountConfig(email="me@gmail.com", provider="google")

    return _Backend(svc)


# ---------------------------------------------------------------------------
# query translation
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "query,expected",
    [
        ("from:alice", "from:alice"),
        ("subject:meeting", "subject:meeting"),
        ("body:invoice", "invoice"),
        ("standup", "standup"),
        ("meeting | standup", "(meeting OR standup)"),
        ("meeting OR standup", "(meeting OR standup)"),
        ("from:alice | from:bob", "(from:alice OR from:bob)"),
        ("subject:quarterly report", 'subject:"quarterly report"'),
    ],
)
def test_translate_query_cases(query, expected):
    assert _translate_query(query) == expected


def test_translate_query_with_dates():
    q = _translate_query("from:alice", from_date="2026-01-01", to_date="2026-02-01")
    assert q == "from:alice after:2026/01/01 before:2026/02/01"


def test_translate_query_or_group_with_date():
    q = _translate_query("a | b", from_date="2026-01-01")
    assert q == "(a OR b) after:2026/01/01"


def test_date_to_gmail_accepts_iso_datetime():
    assert _date_to_gmail("2026-07-05T12:00:00") == "2026/07/05"
    assert _date_to_gmail("2026-07-05") == "2026/07/05"


# ---------------------------------------------------------------------------
# label mapping
# ---------------------------------------------------------------------------


def test_folder_filter_system_labels():
    svc = FakeGmailService()
    be = make_backend(svc)
    assert be._folder_filter(svc, "inbox") == (["INBOX"], "")
    assert be._folder_filter(svc, "sent") == (["SENT"], "")
    assert be._folder_filter(svc, "trash") == (["TRASH"], "")
    assert be._folder_filter(svc, "junk") == (["SPAM"], "")
    assert be._folder_filter(svc, "drafts") == (["DRAFT"], "")


def test_folder_filter_archive_is_query_not_label():
    svc = FakeGmailService()
    be = make_backend(svc)
    label_ids, extra_q = be._folder_filter(svc, "archive")
    assert label_ids == []
    assert extra_q == "-in:inbox -in:trash -in:spam"


def test_folder_filter_unread_adds_label():
    svc = FakeGmailService()
    be = make_backend(svc)
    assert be._folder_filter(svc, "inbox", unread=True) == (["INBOX", "UNREAD"], "")


def test_resolve_user_label_read_missing_raises():
    svc = FakeGmailService()
    be = make_backend(svc)
    with pytest.raises(ValueError):
        be._resolve_label_id(svc, "Projects", create=False)


def test_resolve_user_label_created_on_write():
    svc = FakeGmailService()
    be = make_backend(svc)
    label_id = be._resolve_label_id(svc, "Projects", create=True)
    assert label_id == "Label_1"
    # Existing label resolves by name without creating a second one.
    again = be._resolve_label_id(svc, "projects", create=True)
    assert again == "Label_1"


# ---------------------------------------------------------------------------
# list / get shape parity
# ---------------------------------------------------------------------------


def test_list_messages_shape_and_labels():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", subject="A", labels=["INBOX", "UNREAD"])
    svc.messages["m2"] = make_message("m2", subject="B", labels=["INBOX"])
    svc.messages["s1"] = make_message("s1", subject="Sent", labels=["SENT"])
    be = make_backend(svc)

    result = be.list_messages(folder="inbox", limit=10)
    assert [m["id"] for m in result] == ["m1", "m2"]
    for m in result:
        assert set(m.keys()) == LIST_KEYS
        assert m["changekey"] is None
    assert result[0]["is_read"] is False  # m1 has UNREAD
    assert result[1]["is_read"] is True


def test_list_messages_addresses_and_datetime():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message(
        "m1",
        frm="Alice <alice@example.com>",
        to="me@gmail.com, bob@example.com",
        cc="carol@example.com",
    )
    be = make_backend(svc)
    m = be.list_messages()[0]
    assert m["from"] == "alice@example.com"
    assert m["to"] == ["me@gmail.com", "bob@example.com"]
    assert m["cc"] == ["carol@example.com"]
    expected = datetime.fromtimestamp(1700000000, tz=timezone.utc).isoformat()
    assert m["datetime_received"] == expected


def test_get_message_prefers_plain_body():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message(
        "m1", plain="plain text body", html="<p>html body</p>"
    )
    be = make_backend(svc)
    result = be.get_message("m1")
    assert set(result.keys()) == GET_KEYS
    assert result["body"] == "plain text body"
    assert result["body_type"] == "text"


def test_get_message_html_fallback_stripped():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message(
        "m1", plain=None, html="<p>Hello <b>world</b></p>"
    )
    be = make_backend(svc)
    result = be.get_message("m1")
    assert result["body_type"] == "html"
    assert "Hello" in result["body"] and "<b>" not in result["body"]


def test_get_message_not_found_returns_error():
    svc = FakeGmailService()
    be = make_backend(svc)
    result = be.get_message("missing")
    assert "error" in result


def test_batch_get_messages_none_for_missing():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", plain="body one")
    be = make_backend(svc)
    results = be.batch_get_messages(["m1", "missing"])
    assert len(results) == 2
    assert set(results[0].keys()) == GET_KEYS
    assert results[1] is None


def test_has_attachments_flag():
    att = {"mimeType": "application/pdf", "filename": "doc.pdf",
           "body": {"attachmentId": "att1", "size": 10}}
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", plain="x", attachments=[att])
    svc.messages["m2"] = make_message("m2", plain="x")
    be = make_backend(svc)
    by_id = {m["id"]: m for m in be.list_messages()}
    assert by_id["m1"]["has_attachments"] is True
    assert by_id["m2"]["has_attachments"] is False


# ---------------------------------------------------------------------------
# datetime helper
# ---------------------------------------------------------------------------


def test_internaldate_to_iso():
    assert _internaldate_to_iso(None) is None
    iso = _internaldate_to_iso("1700000000000")
    assert iso.endswith("+00:00")


def test_extract_body_empty():
    assert _extract_body({"headers": []}) == ("", "text")


# ---------------------------------------------------------------------------
# send / MIME construction
# ---------------------------------------------------------------------------


def test_send_message_mime_headers_and_attachment():
    svc = FakeGmailService()
    be = make_backend(svc)
    attachment_bytes = b"PDF-CONTENT"
    result = be.send_message(
        {
            "to": ["bob@example.com", "carol@example.com"],
            "cc": ["dan@example.com"],
            "subject": "Quarterly",
            "body": "See attached.",
            "attachments": [
                {"name": "report.txt", "content": attachment_bytes}
            ],
        }
    )
    assert result["success"] is True
    assert result["to"] == ["bob@example.com", "carol@example.com"]
    assert result["attachments"] == ["report.txt"]

    send_call = [c for c in svc.calls if c[0] == "messages.send"][0]
    raw_b64 = send_call[1]["body"]["raw"]
    raw = base64.urlsafe_b64decode(raw_b64.encode())
    msg = email.message_from_bytes(raw)
    assert msg["To"] == "bob@example.com, carol@example.com"
    assert msg["Cc"] == "dan@example.com"
    assert msg["Subject"] == "Quarterly"

    payloads = {}
    for part in msg.walk():
        fn = part.get_filename()
        if fn:
            payloads[fn] = part.get_payload(decode=True)
    assert payloads.get("report.txt") == attachment_bytes


def test_send_message_scheduled_not_supported():
    svc = FakeGmailService()
    be = make_backend(svc)
    with pytest.raises(BackendNotSupported):
        be.send_message({"to": ["x@y.com"], "subject": "s",
                         "schedule_at": "2026-07-06T10:00:00"})


def test_send_reply_sets_threading_headers():
    svc = FakeGmailService()
    svc.messages["orig"] = make_message(
        "orig",
        subject="Original",
        plain="original body",
        message_id_header="<orig@mail>",
    )
    be = make_backend(svc)
    be.send_message(
        {
            "to": ["alice@example.com"],
            "subject": "",
            "body": "my reply",
            "reply_to_id": "orig",
        }
    )
    send_call = [c for c in svc.calls if c[0] == "messages.send"][0]
    body = send_call[1]["body"]
    assert body["threadId"] == "t_orig"
    raw = base64.urlsafe_b64decode(body["raw"].encode())
    msg = email.message_from_bytes(raw)
    assert msg["In-Reply-To"] == "<orig@mail>"
    assert msg["Subject"] == "Re: Original"


# ---------------------------------------------------------------------------
# draft roundtrip
# ---------------------------------------------------------------------------


def test_draft_roundtrip():
    svc = FakeGmailService()
    be = make_backend(svc)

    created = be.save_draft(
        {"to": ["bob@example.com"], "subject": "Draft subj", "body": "hi"}
    )
    assert created["success"] is True
    assert created["changekey"] is None
    draft_id = created["id"]
    assert draft_id in svc.drafts

    updated = be.update_draft(draft_id, {"subject": "New subj"})
    assert updated["success"] is True
    assert updated["subject"] == "New subj"
    # The body/recipients are preserved through the update.
    raw = base64.urlsafe_b64decode(
        svc.drafts[draft_id]["message"]["raw"].encode()
    )
    msg = email.message_from_bytes(raw)
    assert msg["Subject"] == "New subj"
    assert msg["To"] == "bob@example.com"

    deleted = be.delete_draft(draft_id)
    assert deleted == {"success": True, "id": draft_id}
    assert draft_id not in svc.drafts


def test_update_draft_missing():
    svc = FakeGmailService()
    be = make_backend(svc)
    result = be.update_draft("nope", {"subject": "x"})
    assert result["success"] is False


# ---------------------------------------------------------------------------
# mark / move label diffs
# ---------------------------------------------------------------------------


def test_batch_mark_read_removes_unread_label():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX", "UNREAD"])
    svc.messages["m2"] = make_message("m2", labels=["INBOX", "UNREAD"])
    be = make_backend(svc)

    result = be.batch_mark_messages(folder="inbox", read=True, ids=["m1", "m2"])
    assert result["updated_count"] == 2
    assert result["matched_count"] == 2
    modify_call = [c for c in svc.calls if c[0] == "messages.batchModify"][0]
    body = modify_call[1]["body"]
    assert body["removeLabelIds"] == ["UNREAD"]
    assert body["addLabelIds"] == []
    assert set(body["ids"]) == {"m1", "m2"}


def test_batch_mark_unread_adds_unread_label():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    be = make_backend(svc)
    be.batch_mark_messages(folder="inbox", read=False, ids=["m1"])
    body = [c for c in svc.calls if c[0] == "messages.batchModify"][0][1]["body"]
    assert body["addLabelIds"] == ["UNREAD"]
    assert body["removeLabelIds"] == []


def test_batch_mark_dry_run_does_not_modify():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX", "UNREAD"])
    be = make_backend(svc)
    result = be.batch_mark_messages(
        folder="inbox", read=True, ids=["m1"], dry_run=True
    )
    assert result["updated_count"] == 0
    assert result["matched_count"] == 1
    assert not [c for c in svc.calls if c[0] == "messages.batchModify"]


def test_move_message_label_diff():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    svc.labels.append({"id": "Label_9", "name": "Projects"})
    be = make_backend(svc)

    result = be.move_message("m1", target_folder="Projects", source_folder="inbox")
    assert result["success"] is True
    assert result["new_id"] == "m1"
    body = [c for c in svc.calls if c[0] == "messages.modify"][0][1]["body"]
    assert body["addLabelIds"] == ["Label_9"]
    assert body["removeLabelIds"] == ["INBOX"]


def test_move_message_to_archive_removes_inbox():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    be = make_backend(svc)
    be.move_message("m1", target_folder="archive", source_folder="inbox")
    body = [c for c in svc.calls if c[0] == "messages.modify"][0][1]["body"]
    assert body["addLabelIds"] == []
    assert body["removeLabelIds"] == ["INBOX"]


def test_move_message_to_trash_uses_trash_endpoint():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    be = make_backend(svc)
    result = be.move_message("m1", target_folder="trash")
    assert result["success"] is True
    assert [c for c in svc.calls if c[0] == "messages.trash"]


def test_move_message_missing_target_no_create():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    be = make_backend(svc)
    result = be.move_message("m1", target_folder="Nope", create_folder=False)
    assert result["success"] is False


# ---------------------------------------------------------------------------
# delete / spam / empty
# ---------------------------------------------------------------------------


def test_delete_message_trash_and_permanent():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1")
    be = make_backend(svc)
    assert be.delete_message("m1")["action"] == "moved_to_trash"
    assert be.delete_message("m1", permanent=True)["action"] == "deleted"


def test_mark_as_spam_label_diff():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    be = make_backend(svc)
    result = be.mark_as_spam("m1", is_spam=True, move_to_junk=True)
    assert result["moved_to"] == "junk"
    body = [c for c in svc.calls if c[0] == "messages.modify"][0][1]["body"]
    assert body["addLabelIds"] == ["SPAM"]
    assert body["removeLabelIds"] == ["INBOX"]


def test_empty_folder_batch_deletes():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["TRASH"])
    svc.messages["m2"] = make_message("m2", labels=["TRASH"])
    be = make_backend(svc)
    result = be.empty_folder("trash")
    assert result["deleted_count"] == 2
    body = [c for c in svc.calls if c[0] == "messages.batchDelete"][0][1]["body"]
    assert set(body["ids"]) == {"m1", "m2"}


# ---------------------------------------------------------------------------
# attachments
# ---------------------------------------------------------------------------


def test_list_and_download_attachment(tmp_path):
    att = {"mimeType": "application/pdf", "filename": "doc.pdf",
           "body": {"attachmentId": "att1", "size": 11}}
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", plain="body", attachments=[att])
    svc.attachments["att1"] = b"PDF-CONTENT"
    be = make_backend(svc)

    listing = be.list_attachments("m1")
    assert listing == [
        {"index": 0, "name": "doc.pdf", "size": 11, "content_type": "application/pdf"}
    ]

    out = tmp_path / "out.pdf"
    result = be.download_attachment("m1", 0, str(out))
    assert result["success"] is True
    assert out.read_bytes() == b"PDF-CONTENT"
    assert result["size"] == 11


# ---------------------------------------------------------------------------
# search delegates to list with translated query
# ---------------------------------------------------------------------------


def test_search_messages_passes_translated_query():
    svc = FakeGmailService()
    svc.messages["m1"] = make_message("m1", labels=["INBOX"])
    be = make_backend(svc)
    be.search_messages("from:alice", folder="inbox")
    list_call = [c for c in svc.calls if c[0] == "messages.list"][0]
    assert list_call[1]["q"] == "from:alice"
    assert list_call[1]["labelIds"] == ["INBOX"]


def test_search_messages_archive_combines_query():
    svc = FakeGmailService()
    be = make_backend(svc)
    be.search_messages("invoice", folder="archive")
    list_call = [c for c in svc.calls if c[0] == "messages.list"][0]
    assert list_call[1]["q"] == "invoice -in:inbox -in:trash -in:spam"
    assert list_call[1]["labelIds"] is None
