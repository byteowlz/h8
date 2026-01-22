"""Mail operations."""

import os
import email
from email.mime.text import MIMEText
from email.mime.multipart import MIMEMultipart
from datetime import datetime
from typing import Optional
from zoneinfo import ZoneInfo

from exchangelib import Message, Mailbox, HTMLBody, ExtendedProperty, EWSDateTime
from exchangelib.account import Account
from exchangelib.items import HARD_DELETE


# Extended property for deferred/scheduled sending
# PR_DEFERRED_SEND_TIME (0x3FEF / 16367)
class DeferredSendTime(ExtendedProperty):
    """MAPI property for scheduling email delivery."""

    property_tag = 0x3FEF
    property_type = "SystemTime"


# Register the extended property on Message class
Message.register("deferred_send_time", DeferredSendTime)


FOLDER_MAP = {
    "inbox": "inbox",
    "sent": "sent",
    "drafts": "drafts",
    "trash": "trash",
    "junk": "junk",
}


def get_folder(account: Account, folder_name: str):
    """Get a folder by name."""
    name = folder_name.lower()
    if name == "inbox":
        return account.inbox
    elif name == "sent":
        return account.sent
    elif name == "drafts":
        return account.drafts
    elif name == "trash":
        return account.trash
    elif name == "junk":
        return account.junk
    else:
        # Try to find by name in all folders
        for folder in account.root.walk():
            if folder.name.lower() == name:
                return folder
        raise ValueError(f"Folder '{folder_name}' not found")


def list_messages(
    account: Account,
    folder: str = "inbox",
    limit: int = 20,
    unread: bool = False,
) -> list[dict]:
    """List messages in a folder."""
    mail_folder = get_folder(account, folder)

    query = mail_folder.all()
    if unread:
        query = query.filter(is_read=False)

    # Use .only() to fetch only required fields - avoids fetching large bodies
    query = query.order_by("-datetime_received").only(
        "id",
        "changekey",
        "subject",
        "sender",
        "to_recipients",
        "cc_recipients",
        "datetime_received",
        "is_read",
        "has_attachments",
    )[:limit]

    messages = []
    for item in query:
        if not hasattr(item, "subject"):
            continue

        messages.append(
            {
                "id": item.id,
                "changekey": item.changekey,
                "subject": item.subject,
                "from": item.sender.email_address if item.sender else None,
                "to": [r.email_address for r in (item.to_recipients or [])],
                "cc": [r.email_address for r in (item.cc_recipients or [])],
                "datetime_received": item.datetime_received.isoformat()
                if item.datetime_received
                else None,
                "is_read": item.is_read,
                "has_attachments": item.has_attachments,
            }
        )

    return messages


def get_message(account: Account, item_id: str, folder: str = "inbox") -> dict:
    """Get a full message by ID including body."""
    from exchangelib import ItemId

    # Fetch item by ID using account.fetch() - EWS IDs are globally unique
    # Note: folder parameter is unused since EWS IDs are unique across folders
    try:
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"error": "Message not found"}

        item = items[0]
        return {
            "id": item.id,
            "changekey": item.changekey,
            "subject": item.subject,
            "from": item.sender.email_address if item.sender else None,
            "to": [r.email_address for r in (item.to_recipients or [])],
            "cc": [r.email_address for r in (item.cc_recipients or [])],
            "datetime_received": item.datetime_received.isoformat()
            if item.datetime_received
            else None,
            "is_read": item.is_read,
            "has_attachments": item.has_attachments,
            "body": item.body,
            "body_type": "html" if isinstance(item.body, HTMLBody) else "text",
        }
    except Exception as e:
        return {"error": f"Failed to fetch message: {e}"}


def fetch_messages(
    account: Account,
    folder: str,
    output_dir: str,
    format: str = "maildir",
    limit: Optional[int] = None,
) -> dict:
    """Fetch messages and save to maildir or mbox format."""
    mail_folder = get_folder(account, folder)

    if format == "maildir":
        return _fetch_to_maildir(mail_folder, output_dir, limit)
    elif format == "mbox":
        return _fetch_to_mbox(mail_folder, output_dir, limit)
    else:
        return {"error": f"Unknown format: {format}"}


def _fetch_to_maildir(mail_folder, output_dir: str, limit: Optional[int]) -> dict:
    """Save messages to Maildir format."""
    # Create Maildir structure
    cur_dir = os.path.join(output_dir, "cur")
    new_dir = os.path.join(output_dir, "new")
    tmp_dir = os.path.join(output_dir, "tmp")

    os.makedirs(cur_dir, exist_ok=True)
    os.makedirs(new_dir, exist_ok=True)
    os.makedirs(tmp_dir, exist_ok=True)

    query = mail_folder.all().order_by("-datetime_received")
    if limit:
        query = query[:limit]

    count = 0
    for item in query:
        if not hasattr(item, "subject"):
            continue

        # Build email message
        msg = _item_to_email(item)

        # Generate filename
        timestamp = (
            item.datetime_received.timestamp()
            if item.datetime_received
            else datetime.now().timestamp()
        )
        flags = "S" if item.is_read else ""
        filename = f"{int(timestamp)}.{item.id[:20]}.h8:2,{flags}"

        # Save to cur or new based on read status
        target_dir = cur_dir if item.is_read else new_dir
        filepath = os.path.join(target_dir, filename)

        with open(filepath, "w") as f:
            f.write(msg.as_string())

        count += 1

    return {"success": True, "count": count, "output": output_dir}


def _fetch_to_mbox(mail_folder, output_dir: str, limit: Optional[int]) -> dict:
    """Save messages to mbox format."""
    import mailbox

    os.makedirs(output_dir, exist_ok=True)
    mbox_path = os.path.join(output_dir, "mail.mbox")

    mbox = mailbox.mbox(mbox_path)
    mbox.lock()

    query = mail_folder.all().order_by("-datetime_received")
    if limit:
        query = query[:limit]

    count = 0
    try:
        for item in query:
            if not hasattr(item, "subject"):
                continue

            msg = _item_to_email(item)
            mbox.add(msg)
            count += 1
    finally:
        mbox.unlock()
        mbox.close()

    return {"success": True, "count": count, "output": mbox_path}


def _item_to_email(item) -> email.message.EmailMessage:
    """Convert an EWS item to an email.message.EmailMessage."""
    msg = MIMEMultipart("alternative")

    msg["Subject"] = item.subject or ""
    msg["From"] = item.sender.email_address if item.sender else ""
    msg["To"] = ", ".join(r.email_address for r in (item.to_recipients or []))
    if item.cc_recipients:
        msg["Cc"] = ", ".join(r.email_address for r in item.cc_recipients)
    if item.datetime_received:
        msg["Date"] = item.datetime_received.strftime("%a, %d %b %Y %H:%M:%S %z")
    msg["Message-ID"] = f"<{item.id}@ews>"

    # Add body
    if item.body:
        if isinstance(item.body, HTMLBody):
            msg.attach(MIMEText(str(item.body), "html"))
        else:
            msg.attach(MIMEText(str(item.body), "plain"))

    return msg


def send_message(account: Account, message_data: dict) -> dict:
    """Send an email message, optionally scheduled for later delivery.

    Args:
        account: EWS account
        message_data: Dict with keys:
            - to: list of recipient emails (required)
            - subject: email subject (required)
            - body: email body text
            - cc: list of CC recipients
            - html: if True, body is HTML
            - schedule_at: ISO datetime string for delayed delivery (optional)

    Returns:
        Dict with success status and message info
    """
    to_recipients = [Mailbox(email_address=addr) for addr in message_data["to"]]
    cc_recipients = [Mailbox(email_address=addr) for addr in message_data.get("cc", [])]

    body = message_data.get("body", "")
    if message_data.get("html", False):
        body = HTMLBody(body)

    msg = Message(
        account=account,
        subject=message_data["subject"],
        body=body,
        to_recipients=to_recipients,
        cc_recipients=cc_recipients if cc_recipients else None,
    )

    # Handle scheduled/deferred sending
    schedule_at = message_data.get("schedule_at")
    if schedule_at:
        # Parse the datetime and set the deferred send time
        tz = ZoneInfo("Europe/Berlin")
        if isinstance(schedule_at, str):
            send_time = datetime.fromisoformat(schedule_at)
            if send_time.tzinfo is None:
                send_time = send_time.replace(tzinfo=tz)
            else:
                # Convert to our target timezone to avoid offset-based timezone issues
                send_time = send_time.astimezone(tz)
        else:
            send_time = schedule_at

        # Convert to EWSDateTime - need to use an EWSTimeZone
        from exchangelib import EWSTimeZone

        ews_tz = EWSTimeZone("Europe/Berlin")
        ews_send_time = EWSDateTime(
            send_time.year,
            send_time.month,
            send_time.day,
            send_time.hour,
            send_time.minute,
            send_time.second,
            tzinfo=ews_tz,
        )
        msg.deferred_send_time = ews_send_time

        # For scheduled messages, we need to save to drafts first, then send
        # The deferred_send_time property tells Exchange when to actually deliver
        msg.send_and_save()

        return {
            "success": True,
            "scheduled": True,
            "schedule_at": send_time.isoformat(),
            "subject": message_data["subject"],
            "to": message_data["to"],
        }

    # Immediate send
    msg.send()

    return {
        "success": True,
        "subject": message_data["subject"],
        "to": message_data["to"],
    }


def save_draft(account: Account, draft_data: dict) -> dict:
    """Save a new draft to Exchange drafts folder.

    Args:
        account: EWS account
        draft_data: Dict with keys: to, cc, bcc, subject, body, html, in_reply_to, references

    Returns:
        Dict with id, changekey, and success status
    """
    to_recipients = [Mailbox(email_address=addr) for addr in draft_data.get("to", [])]
    cc_recipients = [Mailbox(email_address=addr) for addr in draft_data.get("cc", [])]
    bcc_recipients = [Mailbox(email_address=addr) for addr in draft_data.get("bcc", [])]

    body = draft_data.get("body", "")
    if draft_data.get("html", False):
        body = HTMLBody(body)

    msg = Message(
        account=account,
        folder=account.drafts,
        subject=draft_data.get("subject", ""),
        body=body,
        to_recipients=to_recipients if to_recipients else None,
        cc_recipients=cc_recipients if cc_recipients else None,
        bcc_recipients=bcc_recipients if bcc_recipients else None,
    )

    # Set reply headers if provided
    if draft_data.get("in_reply_to"):
        msg.in_reply_to = draft_data["in_reply_to"]
    if draft_data.get("references"):
        msg.references = draft_data["references"]

    # Save the draft
    msg.save()

    return {
        "success": True,
        "id": msg.id,
        "changekey": msg.changekey,
        "subject": draft_data.get("subject", ""),
    }


def update_draft(account: Account, item_id: str, update_data: dict) -> dict:
    """Update an existing draft.

    Args:
        account: EWS account
        item_id: The item ID of the draft to update
        update_data: Dict with fields to update (to, cc, bcc, subject, body, html)

    Returns:
        Dict with updated id, changekey, and success status
    """
    # Find the draft
    drafts_folder = account.drafts
    draft = None

    for item in drafts_folder.all():
        if item.id == item_id:
            draft = item
            break

    if draft is None:
        return {"success": False, "error": f"Draft not found: {item_id}"}

    # Update fields
    if "to" in update_data:
        draft.to_recipients = [
            Mailbox(email_address=addr) for addr in update_data["to"]
        ]
    if "cc" in update_data:
        draft.cc_recipients = [
            Mailbox(email_address=addr) for addr in update_data["cc"]
        ]
    if "bcc" in update_data:
        draft.bcc_recipients = [
            Mailbox(email_address=addr) for addr in update_data["bcc"]
        ]
    if "subject" in update_data:
        draft.subject = update_data["subject"]
    if "body" in update_data:
        body = update_data["body"]
        if update_data.get("html", False):
            body = HTMLBody(body)
        draft.body = body

    # Save the changes
    draft.save()

    return {
        "success": True,
        "id": draft.id,
        "changekey": draft.changekey,
        "subject": draft.subject,
    }


def delete_draft(account: Account, item_id: str) -> dict:
    """Delete a draft.

    Args:
        account: EWS account
        item_id: The item ID of the draft to delete

    Returns:
        Dict with success status
    """
    # Find the draft
    drafts_folder = account.drafts
    draft = None

    for item in drafts_folder.all():
        if item.id == item_id:
            draft = item
            break

    if draft is None:
        return {"success": False, "error": f"Draft not found: {item_id}"}

    # Delete the draft (move to trash by default, or hard delete)
    draft.delete()

    return {
        "success": True,
        "id": item_id,
    }


def list_drafts(account: Account, limit: int = 20) -> list[dict]:
    """List drafts in the drafts folder.

    Args:
        account: EWS account
        limit: Maximum number of drafts to return

    Returns:
        List of draft dictionaries
    """
    drafts_folder = account.drafts

    query = (
        drafts_folder.all()
        .order_by("-last_modified_time")
        .only(
            "id",
            "changekey",
            "subject",
            "to_recipients",
            "cc_recipients",
            "last_modified_time",
        )[:limit]
    )

    drafts = []
    for item in query:
        if not hasattr(item, "subject"):
            continue

        drafts.append(
            {
                "id": item.id,
                "changekey": item.changekey,
                "subject": item.subject,
                "to": [r.email_address for r in (item.to_recipients or [])],
                "cc": [r.email_address for r in (item.cc_recipients or [])],
                "last_modified": item.last_modified_time.isoformat()
                if item.last_modified_time
                else None,
            }
        )

    return drafts


def list_attachments(
    account: Account, item_id: str, folder: str = "inbox"
) -> list[dict]:
    """List attachments for a message.

    Args:
        account: EWS account
        item_id: The item ID of the message
        folder: Folder containing the message

    Returns:
        List of attachment dictionaries with id, name, size, content_type
    """
    from exchangelib import ItemId

    try:
        # Fetch item by ID using account.fetch() - EWS IDs are globally unique
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return []

        item = items[0]
        if not item.has_attachments or not item.attachments:
            return []

        attachments = []
        for i, att in enumerate(item.attachments):
            attachments.append(
                {
                    "index": i,
                    "name": att.name or f"attachment_{i}",
                    "size": getattr(att, "size", None),
                    "content_type": getattr(
                        att, "content_type", "application/octet-stream"
                    ),
                }
            )
        return attachments
    except Exception:
        return []


def batch_get_messages(
    account: Account,
    item_ids: list[str],
    folder: str = "inbox",
) -> list[dict]:
    """Fetch multiple messages by ID in a single batch request.

    This is much more efficient than calling get_message() multiple times
    as it uses a single EWS GetItem request to fetch all messages.

    Args:
        account: EWS account
        item_ids: List of message IDs to fetch
        folder: Folder containing the messages (unused, IDs are global in EWS)

    Returns:
        List of message dictionaries (in same order as item_ids, with None for not found)
    """
    from exchangelib import ItemId

    if not item_ids:
        return []

    # Create ItemId objects for bulk fetch
    # EWS item IDs are globally unique, so we don't need the folder
    ids = [ItemId(id=item_id) for item_id in item_ids]

    try:
        # Use account.fetch() which sends a single GetItem request for all IDs
        # This is the correct way to batch fetch items by ID in exchangelib
        items_by_id = {}
        for item in account.fetch(ids=ids):
            # fetch() may return None for items that don't exist or are inaccessible
            if item is not None and hasattr(item, "id"):
                items_by_id[item.id] = item

        # Return results in the same order as requested
        results = []
        for item_id in item_ids:
            item = items_by_id.get(item_id)
            if item is None:
                results.append(None)
            else:
                results.append(
                    {
                        "id": item.id,
                        "changekey": item.changekey,
                        "subject": item.subject,
                        "from": item.sender.email_address if item.sender else None,
                        "to": [r.email_address for r in (item.to_recipients or [])],
                        "cc": [r.email_address for r in (item.cc_recipients or [])],
                        "datetime_received": item.datetime_received.isoformat()
                        if item.datetime_received
                        else None,
                        "is_read": item.is_read,
                        "has_attachments": item.has_attachments,
                        "body": str(item.body) if item.body else "",
                        "body_type": "html"
                        if isinstance(item.body, HTMLBody)
                        else "text",
                    }
                )
        return results
    except Exception as e:
        # Fall back to individual fetches if batch fails
        results = []
        for item_id in item_ids:
            msg = get_message(account, item_id, folder)
            results.append(msg if "error" not in msg else None)
        return results


def search_messages(
    account: Account,
    query: str,
    folder: str = "inbox",
    limit: int = 50,
) -> list[dict]:
    """Search messages by subject or sender.

    Searches subject and sender fields using case-insensitive contains.

    Args:
        account: EWS account
        query: Search query string
        folder: Folder to search in (default: inbox)
        limit: Maximum number of results to return

    Returns:
        List of matching message dictionaries
    """
    mail_folder = get_folder(account, folder)

    # Use subject__icontains for reliable cross-server compatibility
    # QueryString search isn't supported on all Exchange configurations
    results = (
        mail_folder.filter(subject__icontains=query)
        .order_by("-datetime_received")
        .only(
            "id",
            "changekey",
            "subject",
            "sender",
            "to_recipients",
            "cc_recipients",
            "datetime_received",
            "is_read",
            "has_attachments",
        )[:limit]
    )

    messages = []
    for item in results:
        if not hasattr(item, "subject"):
            continue

        messages.append(
            {
                "id": item.id,
                "changekey": item.changekey,
                "subject": item.subject,
                "from": item.sender.email_address if item.sender else None,
                "to": [r.email_address for r in (item.to_recipients or [])],
                "cc": [r.email_address for r in (item.cc_recipients or [])],
                "datetime_received": item.datetime_received.isoformat()
                if item.datetime_received
                else None,
                "is_read": item.is_read,
                "has_attachments": item.has_attachments,
            }
        )

    return messages


def download_attachment(
    account: Account,
    item_id: str,
    attachment_index: int,
    output_path: str,
    folder: str = "inbox",
) -> dict:
    """Download a specific attachment from a message.

    Args:
        account: EWS account
        item_id: The item ID of the message
        attachment_index: Index of the attachment to download
        output_path: Path to save the attachment
        folder: Folder containing the message

    Returns:
        Dict with success status and file path
    """
    from exchangelib import ItemId

    try:
        # Fetch item by ID using account.fetch() - EWS IDs are globally unique
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"success": False, "error": "Message not found"}

        item = items[0]
        if not item.has_attachments or not item.attachments:
            return {"success": False, "error": "Message has no attachments"}

        if attachment_index < 0 or attachment_index >= len(item.attachments):
            return {
                "success": False,
                "error": f"Invalid attachment index: {attachment_index}",
            }

        att = item.attachments[attachment_index]

        # Determine output file path
        filename = att.name or f"attachment_{attachment_index}"
        if os.path.isdir(output_path):
            filepath = os.path.join(output_path, filename)
        else:
            filepath = output_path

        # Write attachment content
        with open(filepath, "wb") as f:
            f.write(att.content)

        return {
            "success": True,
            "path": filepath,
            "name": filename,
            "size": len(att.content),
        }
    except Exception as e:
        return {"success": False, "error": f"Failed to download attachment: {e}"}


def delete_message(
    account: Account,
    item_id: str,
    folder: str = "inbox",
    permanent: bool = False,
) -> dict:
    """Delete a message (move to trash or permanently delete).

    Args:
        account: EWS account
        item_id: The item ID of the message to delete
        folder: Folder containing the message (unused, IDs are global in EWS)
        permanent: If True, permanently delete; if False, move to trash

    Returns:
        Dict with success status
    """
    from exchangelib import ItemId

    try:
        # Fetch the item by ID using account.fetch()
        # EWS item IDs are globally unique, so we don't need the folder
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"success": False, "error": f"Message not found: {item_id}"}

        item = items[0]

        if permanent:
            # Hard delete - permanently remove
            item.delete()
            return {"success": True, "id": item_id, "action": "deleted"}
        else:
            # Soft delete - move to trash (Deleted Items)
            item.move_to_trash()
            return {"success": True, "id": item_id, "action": "moved_to_trash"}
    except Exception as e:
        return {"success": False, "error": f"Failed to delete message: {e}"}


def move_message(
    account: Account,
    item_id: str,
    target_folder: str,
    source_folder: str = "inbox",
    create_folder: bool = False,
) -> dict:
    """Move a message to another folder.

    Args:
        account: EWS account
        item_id: The item ID of the message to move
        target_folder: Destination folder name or path
        source_folder: Source folder containing the message (unused, IDs are global in EWS)
        create_folder: If True, create target folder if it doesn't exist

    Returns:
        Dict with success status and new item ID
    """
    from exchangelib import ItemId

    # Try to get target folder, optionally create it
    try:
        target = get_folder(account, target_folder)
    except ValueError:
        if create_folder:
            target = _create_folder(account, target_folder)
        else:
            return {
                "success": False,
                "error": f"Target folder not found: {target_folder}. Use --create to create it.",
            }

    try:
        # Fetch the item by ID using account.fetch()
        # EWS item IDs are globally unique, so we don't need the source folder
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"success": False, "error": f"Message not found: {item_id}"}

        item = items[0]
        new_item = item.move(target)

        return {
            "success": True,
            "id": item_id,
            "new_id": new_item.id if new_item else None,
            "target_folder": target_folder,
        }
    except Exception as e:
        return {"success": False, "error": f"Failed to move message: {e}"}


def _create_folder(account: Account, folder_path: str):
    """Create a folder, supporting nested paths like 'inbox/projects/active'.

    Args:
        account: EWS account
        folder_path: Folder path (e.g., 'projects' or 'inbox/projects/active')

    Returns:
        The created folder
    """
    from exchangelib import Folder

    parts = folder_path.split("/")

    # Determine parent folder
    if parts[0].lower() in FOLDER_MAP:
        parent = get_folder(account, parts[0])
        parts = parts[1:]
    else:
        # Create under msg_folder_root (top-level)
        parent = account.msg_folder_root

    # Create nested folders
    current = parent
    for part in parts:
        if not part:
            continue
        # Check if folder exists
        existing = None
        for child in current.children:
            if child.name.lower() == part.lower():
                existing = child
                break

        if existing:
            current = existing
        else:
            # Create new folder
            new_folder = Folder(parent=current, name=part)
            new_folder.save()
            current = new_folder

    return current


def empty_folder(account: Account, folder_name: str = "trash") -> dict:
    """Empty a folder by permanently deleting all items.

    Args:
        account: EWS account
        folder_name: Folder to empty (default: trash)

    Returns:
        Dict with success status and count of deleted items
    """
    try:
        folder = get_folder(account, folder_name)

        # Count items before deletion
        count = folder.total_count or 0

        if count == 0:
            return {"success": True, "deleted_count": 0, "folder": folder_name}

        # Empty the folder - this permanently deletes all items
        # Using delete_type=HARD_DELETE to bypass Recoverable Items
        folder.empty(delete_sub_folders=False, delete_type=HARD_DELETE)

        return {"success": True, "deleted_count": count, "folder": folder_name}
    except ValueError as e:
        return {"success": False, "error": str(e)}
    except Exception as e:
        return {"success": False, "error": f"Failed to empty folder: {e}"}


def mark_as_spam(
    account: Account,
    item_id: str,
    is_spam: bool = True,
    move_to_junk: bool = True,
) -> dict:
    """Mark a message as spam/junk or not spam.

    Args:
        account: EWS account
        item_id: The item ID of the message
        is_spam: True to mark as spam, False to mark as not spam
        move_to_junk: If True and is_spam, move to junk folder; if False and not is_spam, move to inbox

    Returns:
        Dict with success status
    """
    from exchangelib import ItemId

    try:
        # Fetch the item by ID
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"success": False, "error": f"Message not found: {item_id}"}

        item = items[0]

        if is_spam:
            # Mark as junk and optionally move to junk folder
            if move_to_junk:
                item.mark_as_junk(is_junk=True, move_item=True)
                return {
                    "success": True,
                    "id": item_id,
                    "action": "marked_as_spam",
                    "moved_to": "junk",
                }
            else:
                item.mark_as_junk(is_junk=True, move_item=False)
                return {"success": True, "id": item_id, "action": "marked_as_spam"}
        else:
            # Mark as not junk and optionally move to inbox
            if move_to_junk:
                item.mark_as_junk(is_junk=False, move_item=True)
                return {
                    "success": True,
                    "id": item_id,
                    "action": "marked_as_not_spam",
                    "moved_to": "inbox",
                }
            else:
                item.mark_as_junk(is_junk=False, move_item=False)
                return {"success": True, "id": item_id, "action": "marked_as_not_spam"}
    except Exception as e:
        return {"success": False, "error": f"Failed to mark as spam: {e}"}
