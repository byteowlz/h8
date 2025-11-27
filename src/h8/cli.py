"""h8 - EWS CLI for calendar, mail, and contacts."""

import argparse
import json
import sys
from typing import Any

from . import __version__
from .auth import get_account
from . import calendar
from . import mail
from . import contacts
from . import free


DEFAULT_ACCOUNT = 'tommy.falkowski@iem.fraunhofer.de'


def output(data: Any, as_json: bool = True):
    """Output data as JSON or human-readable format."""
    if as_json:
        print(json.dumps(data, indent=2, ensure_ascii=False, default=str))
    else:
        # Simple human-readable output
        if isinstance(data, list):
            for item in data:
                print_item(item)
        elif isinstance(data, dict):
            print_item(data)
        else:
            print(data)


def print_item(item: dict):
    """Print a single item in human-readable format."""
    if 'error' in item:
        print(f"Error: {item['error']}", file=sys.stderr)
        return
    
    if 'subject' in item:  # Calendar or mail
        print(f"- {item.get('subject', 'No subject')}")
        if 'start' in item:
            print(f"  Start: {item['start']}")
            print(f"  End: {item.get('end', 'N/A')}")
            if item.get('location'):
                print(f"  Location: {item['location']}")
        if 'from' in item:
            print(f"  From: {item['from']}")
            print(f"  Date: {item.get('datetime_received', 'N/A')}")
        print()
    elif 'display_name' in item:  # Contact
        print(f"- {item.get('display_name', 'No name')}")
        if item.get('email'):
            print(f"  Email: {item['email']}")
        if item.get('phone'):
            print(f"  Phone: {item['phone']}")
        if item.get('company'):
            print(f"  Company: {item['company']}")
        print()
    elif 'duration_minutes' in item and 'day' in item:  # Free slot
        start = item['start'].split('T')[1][:5] if 'T' in item['start'] else item['start']
        end = item['end'].split('T')[1][:5] if 'T' in item['end'] else item['end']
        print(f"- {item['day']} {item['date']}: {start} - {end} ({item['duration_minutes']} min)")
    elif 'success' in item:
        if item['success']:
            print("Success")
        else:
            print(f"Failed: {item.get('error', 'Unknown error')}", file=sys.stderr)
    else:
        for k, v in item.items():
            print(f"  {k}: {v}")
        print()


def cmd_calendar_list(args):
    """List calendar events."""
    account = get_account(args.account)
    events = calendar.list_events(
        account,
        days=args.days,
        from_date=args.from_date,
        to_date=args.to_date,
    )
    output(events, args.json)


def cmd_calendar_create(args):
    """Create a calendar event."""
    account = get_account(args.account)
    event_data = json.load(sys.stdin)
    result = calendar.create_event(account, event_data)
    output(result, args.json)


def cmd_calendar_delete(args):
    """Delete a calendar event."""
    account = get_account(args.account)
    result = calendar.delete_event(account, args.id)
    output(result, args.json)


def cmd_mail_list(args):
    """List mail messages."""
    account = get_account(args.account)
    messages = mail.list_messages(
        account,
        folder=args.folder,
        limit=args.limit,
        unread=args.unread,
    )
    output(messages, args.json)


def cmd_mail_get(args):
    """Get a full mail message."""
    account = get_account(args.account)
    message = mail.get_message(account, args.id, folder=args.folder)
    output(message, args.json)


def cmd_mail_fetch(args):
    """Fetch mail to maildir/mbox."""
    account = get_account(args.account)
    result = mail.fetch_messages(
        account,
        folder=args.folder,
        output_dir=args.output,
        format=args.format,
        limit=args.limit,
    )
    output(result, args.json)


def cmd_mail_send(args):
    """Send an email."""
    account = get_account(args.account)
    message_data = json.load(sys.stdin)
    result = mail.send_message(account, message_data)
    output(result, args.json)


def cmd_contacts_list(args):
    """List contacts."""
    account = get_account(args.account)
    search = getattr(args, 'search', None)
    contact_list = contacts.list_contacts(account, limit=args.limit, search=search)
    output(contact_list, args.json)


def cmd_contacts_get(args):
    """Get a contact."""
    account = get_account(args.account)
    contact = contacts.get_contact(account, args.id)
    output(contact, args.json)


def cmd_contacts_create(args):
    """Create a contact."""
    account = get_account(args.account)
    contact_data = json.load(sys.stdin)
    result = contacts.create_contact(account, contact_data)
    output(result, args.json)


def cmd_contacts_delete(args):
    """Delete a contact."""
    account = get_account(args.account)
    result = contacts.delete_contact(account, args.id)
    output(result, args.json)


def cmd_free(args):
    """Find free slots in calendar."""
    account = get_account(args.account)
    slots = free.find_free_slots(
        account,
        weeks=args.weeks,
        duration_minutes=args.duration,
        limit=args.limit,
    )
    output(slots, args.json)


def add_common_args(parser):
    """Add common arguments to a parser."""
    parser.add_argument('--json', '-j', action='store_true',
                        help='Output as JSON')
    parser.add_argument('--account', '-a', default=DEFAULT_ACCOUNT,
                        help=f'Email account (default: {DEFAULT_ACCOUNT})')


def main():
    parser = argparse.ArgumentParser(
        prog='h8',
        description='EWS CLI for calendar, mail, and contacts',
    )
    parser.add_argument('--version', action='version', version=f'%(prog)s {__version__}')
    add_common_args(parser)
    
    subparsers = parser.add_subparsers(dest='command', required=True)
    
    # Calendar commands
    cal_parser = subparsers.add_parser('calendar', aliases=['cal'], help='Calendar operations')
    add_common_args(cal_parser)
    cal_subparsers = cal_parser.add_subparsers(dest='subcommand', required=True)
    
    # calendar list
    cal_list = cal_subparsers.add_parser('list', aliases=['ls'], help='List calendar events')
    add_common_args(cal_list)
    cal_list.add_argument('--days', '-d', type=int, default=7, help='Number of days to show')
    cal_list.add_argument('--from', dest='from_date', help='Start date (ISO format)')
    cal_list.add_argument('--to', dest='to_date', help='End date (ISO format)')
    cal_list.set_defaults(func=cmd_calendar_list)
    
    # calendar create
    cal_create = cal_subparsers.add_parser('create', aliases=['new'], help='Create event (JSON from stdin)')
    add_common_args(cal_create)
    cal_create.set_defaults(func=cmd_calendar_create)
    
    # calendar delete
    cal_delete = cal_subparsers.add_parser('delete', aliases=['rm'], help='Delete event')
    add_common_args(cal_delete)
    cal_delete.add_argument('--id', required=True, help='Event ID')
    cal_delete.set_defaults(func=cmd_calendar_delete)
    
    # Mail commands
    mail_parser = subparsers.add_parser('mail', aliases=['m'], help='Mail operations')
    add_common_args(mail_parser)
    mail_subparsers = mail_parser.add_subparsers(dest='subcommand', required=True)
    
    # mail list
    mail_list = mail_subparsers.add_parser('list', aliases=['ls'], help='List messages')
    add_common_args(mail_list)
    mail_list.add_argument('--folder', '-f', default='inbox', help='Folder name')
    mail_list.add_argument('--limit', '-l', type=int, default=20, help='Max messages')
    mail_list.add_argument('--unread', '-u', action='store_true', help='Only unread')
    mail_list.set_defaults(func=cmd_mail_list)
    
    # mail get
    mail_get = mail_subparsers.add_parser('get', help='Get full message')
    add_common_args(mail_get)
    mail_get.add_argument('--id', required=True, help='Message ID')
    mail_get.add_argument('--folder', '-f', default='inbox', help='Folder name')
    mail_get.set_defaults(func=cmd_mail_get)
    
    # mail fetch
    mail_fetch = mail_subparsers.add_parser('fetch', help='Fetch to maildir/mbox')
    add_common_args(mail_fetch)
    mail_fetch.add_argument('--folder', '-f', default='inbox', help='Folder name')
    mail_fetch.add_argument('--output', '-o', required=True, help='Output directory')
    mail_fetch.add_argument('--format', choices=['maildir', 'mbox'], default='maildir')
    mail_fetch.add_argument('--limit', '-l', type=int, help='Max messages')
    mail_fetch.set_defaults(func=cmd_mail_fetch)
    
    # mail send
    mail_send = mail_subparsers.add_parser('send', help='Send email (JSON from stdin)')
    add_common_args(mail_send)
    mail_send.set_defaults(func=cmd_mail_send)
    
    # Contacts commands
    contacts_parser = subparsers.add_parser('contacts', aliases=['c'], help='Contacts operations')
    add_common_args(contacts_parser)
    contacts_subparsers = contacts_parser.add_subparsers(dest='subcommand', required=True)
    
    # contacts list
    contacts_list = contacts_subparsers.add_parser('list', aliases=['ls'], help='List contacts')
    add_common_args(contacts_list)
    contacts_list.add_argument('--limit', '-l', type=int, default=100, help='Max contacts')
    contacts_list.add_argument('--search', '-s', help='Search by name or email')
    contacts_list.set_defaults(func=cmd_contacts_list)
    
    # contacts get
    contacts_get = contacts_subparsers.add_parser('get', help='Get contact')
    add_common_args(contacts_get)
    contacts_get.add_argument('--id', required=True, help='Contact ID')
    contacts_get.set_defaults(func=cmd_contacts_get)
    
    # contacts create
    contacts_create = contacts_subparsers.add_parser('create', aliases=['new'], help='Create contact (JSON from stdin)')
    add_common_args(contacts_create)
    contacts_create.set_defaults(func=cmd_contacts_create)
    
    # contacts delete
    contacts_delete = contacts_subparsers.add_parser('delete', aliases=['rm'], help='Delete contact')
    add_common_args(contacts_delete)
    contacts_delete.add_argument('--id', required=True, help='Contact ID')
    contacts_delete.set_defaults(func=cmd_contacts_delete)
    
    # Free slots command
    free_parser = subparsers.add_parser('free', help='Find free slots in calendar')
    add_common_args(free_parser)
    free_parser.add_argument('--weeks', '-w', type=int, default=1,
                             help='Number of weeks to look at (1 = current week)')
    free_parser.add_argument('--duration', '-d', type=int, default=30,
                             help='Minimum slot duration in minutes')
    free_parser.add_argument('--limit', '-l', type=int,
                             help='Maximum number of slots to return')
    free_parser.set_defaults(func=cmd_free)
    
    args = parser.parse_args()
    
    try:
        args.func(args)
    except Exception as e:
        output({'error': str(e)}, args.json)
        sys.exit(1)


if __name__ == '__main__':
    main()
