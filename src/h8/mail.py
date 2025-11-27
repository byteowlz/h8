"""Mail operations."""

import os
import email
from email.mime.text import MIMEText
from email.mime.multipart import MIMEMultipart
from datetime import datetime
from typing import Optional

from exchangelib import Message, Mailbox, HTMLBody
from exchangelib.account import Account


FOLDER_MAP = {
    'inbox': 'inbox',
    'sent': 'sent',
    'drafts': 'drafts',
    'trash': 'trash',
    'junk': 'junk',
}


def get_folder(account: Account, folder_name: str):
    """Get a folder by name."""
    name = folder_name.lower()
    if name == 'inbox':
        return account.inbox
    elif name == 'sent':
        return account.sent
    elif name == 'drafts':
        return account.drafts
    elif name == 'trash':
        return account.trash
    elif name == 'junk':
        return account.junk
    else:
        # Try to find by name in all folders
        for folder in account.root.walk():
            if folder.name.lower() == name:
                return folder
        raise ValueError(f"Folder '{folder_name}' not found")


def list_messages(
    account: Account,
    folder: str = 'inbox',
    limit: int = 20,
    unread: bool = False,
) -> list[dict]:
    """List messages in a folder."""
    mail_folder = get_folder(account, folder)
    
    query = mail_folder.all()
    if unread:
        query = query.filter(is_read=False)
    
    # Use .only() to fetch only required fields - avoids fetching large bodies
    query = query.order_by('-datetime_received').only(
        'id', 'changekey', 'subject', 'sender', 'to_recipients',
        'cc_recipients', 'datetime_received', 'is_read', 'has_attachments'
    )[:limit]
    
    messages = []
    for item in query:
        if not hasattr(item, 'subject'):
            continue
        
        messages.append({
            'id': item.id,
            'changekey': item.changekey,
            'subject': item.subject,
            'from': item.sender.email_address if item.sender else None,
            'to': [r.email_address for r in (item.to_recipients or [])],
            'cc': [r.email_address for r in (item.cc_recipients or [])],
            'datetime_received': item.datetime_received.isoformat() if item.datetime_received else None,
            'is_read': item.is_read,
            'has_attachments': item.has_attachments,
        })
    
    return messages


def get_message(account: Account, item_id: str, folder: str = 'inbox') -> dict:
    """Get a full message by ID including body."""
    mail_folder = get_folder(account, folder)
    
    # Search in the folder
    for item in mail_folder.all():
        if item.id == item_id:
            return {
                'id': item.id,
                'changekey': item.changekey,
                'subject': item.subject,
                'from': item.sender.email_address if item.sender else None,
                'to': [r.email_address for r in (item.to_recipients or [])],
                'cc': [r.email_address for r in (item.cc_recipients or [])],
                'datetime_received': item.datetime_received.isoformat() if item.datetime_received else None,
                'is_read': item.is_read,
                'has_attachments': item.has_attachments,
                'body': item.body,
                'body_type': 'html' if isinstance(item.body, HTMLBody) else 'text',
            }
    
    return {'error': 'Message not found'}


def fetch_messages(
    account: Account,
    folder: str,
    output_dir: str,
    format: str = 'maildir',
    limit: Optional[int] = None,
) -> dict:
    """Fetch messages and save to maildir or mbox format."""
    mail_folder = get_folder(account, folder)
    
    if format == 'maildir':
        return _fetch_to_maildir(mail_folder, output_dir, limit)
    elif format == 'mbox':
        return _fetch_to_mbox(mail_folder, output_dir, limit)
    else:
        return {'error': f'Unknown format: {format}'}


def _fetch_to_maildir(mail_folder, output_dir: str, limit: Optional[int]) -> dict:
    """Save messages to Maildir format."""
    # Create Maildir structure
    cur_dir = os.path.join(output_dir, 'cur')
    new_dir = os.path.join(output_dir, 'new')
    tmp_dir = os.path.join(output_dir, 'tmp')
    
    os.makedirs(cur_dir, exist_ok=True)
    os.makedirs(new_dir, exist_ok=True)
    os.makedirs(tmp_dir, exist_ok=True)
    
    query = mail_folder.all().order_by('-datetime_received')
    if limit:
        query = query[:limit]
    
    count = 0
    for item in query:
        if not hasattr(item, 'subject'):
            continue
        
        # Build email message
        msg = _item_to_email(item)
        
        # Generate filename
        timestamp = item.datetime_received.timestamp() if item.datetime_received else datetime.now().timestamp()
        flags = 'S' if item.is_read else ''
        filename = f"{int(timestamp)}.{item.id[:20]}.h8:2,{flags}"
        
        # Save to cur or new based on read status
        target_dir = cur_dir if item.is_read else new_dir
        filepath = os.path.join(target_dir, filename)
        
        with open(filepath, 'w') as f:
            f.write(msg.as_string())
        
        count += 1
    
    return {'success': True, 'count': count, 'output': output_dir}


def _fetch_to_mbox(mail_folder, output_dir: str, limit: Optional[int]) -> dict:
    """Save messages to mbox format."""
    import mailbox
    
    os.makedirs(output_dir, exist_ok=True)
    mbox_path = os.path.join(output_dir, 'mail.mbox')
    
    mbox = mailbox.mbox(mbox_path)
    mbox.lock()
    
    query = mail_folder.all().order_by('-datetime_received')
    if limit:
        query = query[:limit]
    
    count = 0
    try:
        for item in query:
            if not hasattr(item, 'subject'):
                continue
            
            msg = _item_to_email(item)
            mbox.add(msg)
            count += 1
    finally:
        mbox.unlock()
        mbox.close()
    
    return {'success': True, 'count': count, 'output': mbox_path}


def _item_to_email(item) -> email.message.EmailMessage:
    """Convert an EWS item to an email.message.EmailMessage."""
    msg = MIMEMultipart('alternative')
    
    msg['Subject'] = item.subject or ''
    msg['From'] = item.sender.email_address if item.sender else ''
    msg['To'] = ', '.join(r.email_address for r in (item.to_recipients or []))
    if item.cc_recipients:
        msg['Cc'] = ', '.join(r.email_address for r in item.cc_recipients)
    if item.datetime_received:
        msg['Date'] = item.datetime_received.strftime('%a, %d %b %Y %H:%M:%S %z')
    msg['Message-ID'] = f"<{item.id}@ews>"
    
    # Add body
    if item.body:
        if isinstance(item.body, HTMLBody):
            msg.attach(MIMEText(str(item.body), 'html'))
        else:
            msg.attach(MIMEText(str(item.body), 'plain'))
    
    return msg


def send_message(account: Account, message_data: dict) -> dict:
    """Send an email message."""
    to_recipients = [Mailbox(email_address=addr) for addr in message_data['to']]
    cc_recipients = [Mailbox(email_address=addr) for addr in message_data.get('cc', [])]
    
    body = message_data.get('body', '')
    if message_data.get('html', False):
        body = HTMLBody(body)
    
    msg = Message(
        account=account,
        subject=message_data['subject'],
        body=body,
        to_recipients=to_recipients,
        cc_recipients=cc_recipients if cc_recipients else None,
    )
    
    msg.send()
    
    return {
        'success': True,
        'subject': message_data['subject'],
        'to': message_data['to'],
    }
