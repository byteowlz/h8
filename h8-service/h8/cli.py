"""h8 - EWS CLI for calendar, mail, and contacts."""

import argparse
import json
import re
import sys
from typing import Any

from . import __version__
from .auth import get_account
from . import calendar
from . import mail
from . import contacts
from . import free
from . import people
from .config import resolve_person_alias, get_config
from .dateparser import parse_datetime, parse_attendees
from .schemas import extracted_event_to_h8, extracted_contact_to_h8


DEFAULT_ACCOUNT = "tommy.falkowski@iem.fraunhofer.de"


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


def format_duration(minutes: int) -> str:
    """Format duration in minutes to human-readable string."""
    if minutes < 60:
        return f"{minutes}m"
    hours = minutes // 60
    mins = minutes % 60
    if mins == 0:
        return f"{hours}h"
    return f"{hours}h {mins}m"


def print_item(item: dict):
    """Print a single item in human-readable format."""
    if "error" in item:
        print(f"Error: {item['error']}", file=sys.stderr)
        return

    if "subject" in item:  # Calendar or mail
        print(f"- {item.get('subject', 'No subject')}")
        if "start" in item:
            print(f"  Start: {item['start']}")
            print(f"  End: {item.get('end', 'N/A')}")
            if item.get("location"):
                print(f"  Location: {item['location']}")
        if "from" in item:
            print(f"  From: {item['from']}")
            print(f"  Date: {item.get('datetime_received', 'N/A')}")
        print()
    elif "display_name" in item:  # Contact
        print(f"- {item.get('display_name', 'No name')}")
        if item.get("email"):
            print(f"  Email: {item['email']}")
        if item.get("phone"):
            print(f"  Phone: {item['phone']}")
        if item.get("company"):
            print(f"  Company: {item['company']}")
        print()
    elif "duration_minutes" in item and "day" in item:  # Free slot
        start = (
            item["start"].split("T")[1][:5] if "T" in item["start"] else item["start"]
        )
        end = item["end"].split("T")[1][:5] if "T" in item["end"] else item["end"]
        duration = format_duration(item["duration_minutes"])
        print(f"  {item['day'][:3]} {item['date']}: {start} - {end} ({duration})")
    elif "success" in item:
        if item["success"]:
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

    # Convert from extraction schema if --extracted flag is set
    if getattr(args, "extracted", False):
        event_data = extracted_event_to_h8(event_data)

    result = calendar.create_event(account, event_data)
    output(result, args.json)


def cmd_calendar_add(args):
    """Add a calendar event with natural language input."""
    config = get_config()
    timezone = config.get("timezone", "Europe/Berlin")

    # Combine all positional args into the input text
    input_text = " ".join(args.input)

    # Parse attendees from the text (separated by "with")
    remaining_text, attendee_aliases = parse_attendees(input_text)

    # Resolve attendee aliases to email addresses
    attendees = []
    for alias in attendee_aliases:
        try:
            email = resolve_person_alias(alias)
            attendees.append(email)
        except ValueError:
            # If not found as alias and not an email, report error
            print(f"Warning: Unknown attendee '{alias}', skipping", file=sys.stderr)

    # The title is everything between the datetime and "with"
    # Parse datetime from the beginning
    parsed = parse_datetime(
        remaining_text,
        default_duration_minutes=args.duration,
        timezone=timezone,
    )

    # Extract the title - everything that's not a date/time indicator
    # Simple heuristic: look for quoted string first, otherwise use remaining text
    title_match = re.search(r'"([^"]+)"', remaining_text)
    if title_match:
        title = title_match.group(1)
    else:
        # Remove date/time related words and use the rest as title
        title_text = remaining_text
        # Remove common datetime keywords
        for pattern in [
            r"\b(today|tomorrow|yesterday|morgen|heute|gestern)\b",
            r"\b(monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
            r"\b(montag|dienstag|mittwoch|donnerstag|freitag|samstag|sonntag)\b",
            r"\b(mon|tue|wed|thu|fri|sat|sun|mo|di|mi|do|fr|sa|so)\b",
            r"\b(next|nächste[rn]?)\b",
            r"\b(at|on|um|am|für|for)\b",
            r"\d{1,2}:\d{2}",
            r"\d{1,2}\s*(am|pm|uhr)",
            r"\d{1,2}\s*[-–]\s*\d{1,2}",
            r"\b(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)\w*\b",
        ]:
            title_text = re.sub(pattern, "", title_text, flags=re.IGNORECASE)
        title = " ".join(title_text.split()).strip()

    if not title:
        title = "Meeting"

    # Build event data
    event_data = {
        "subject": title,
        "start": parsed.start.isoformat(),
        "end": parsed.end.isoformat(),
    }

    if args.location:
        event_data["location"] = args.location

    # Note: EWS attendees require additional handling in calendar.py
    # For now, we just create the event without attendees
    # TODO: Add attendee support to create_event

    account = get_account(args.account)
    result = calendar.create_event(account, event_data)

    # Print human-friendly confirmation
    if not args.json:
        print(f"Created: {title}")
        print(
            f"  When: {parsed.start.strftime('%A, %B %d, %Y at %H:%M')} - {parsed.end.strftime('%H:%M')}"
        )
        if attendees:
            print(f"  With: {', '.join(attendees)}")
        if args.location:
            print(f"  Where: {args.location}")
    else:
        result["attendees"] = attendees
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


def cmd_mail_attachments(args):
    """List or download attachments from a message."""
    account = get_account(args.account)

    if args.download is not None:
        # Download specific attachment
        result = mail.download_attachment(
            account,
            args.id,
            args.download,
            args.output or ".",
            folder=args.folder,
        )
        output(result, args.json)
    else:
        # List attachments
        attachments = mail.list_attachments(account, args.id, folder=args.folder)
        output(attachments, args.json)


def cmd_contacts_list(args):
    """List contacts."""
    account = get_account(args.account)
    search = getattr(args, "search", None)
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

    # Convert from extraction schema if --extracted flag is set
    if getattr(args, "extracted", False):
        contact_data = extracted_contact_to_h8(contact_data)

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


def cmd_ppl_agenda(args):
    """View another person's calendar events."""
    account = get_account(args.account)
    email = resolve_person_alias(args.person)
    events = people.get_person_agenda(
        account,
        email,
        days=args.days,
        from_date=args.from_date,
        to_date=args.to_date,
    )
    output(events, args.json)


def cmd_ppl_free(args):
    """Find free slots in another person's calendar."""
    account = get_account(args.account)
    email = resolve_person_alias(args.person)
    slots = people.get_person_free_slots(
        account,
        email,
        weeks=args.weeks,
        duration_minutes=args.duration,
        limit=args.limit,
    )
    output(slots, args.json)


def cmd_ppl_common(args):
    """Find common free slots between multiple people."""
    account = get_account(args.account)
    emails = [resolve_person_alias(p) for p in args.people]
    slots = people.find_common_free_slots(
        account,
        emails,
        weeks=args.weeks,
        duration_minutes=args.duration,
        limit=args.limit,
    )
    output(slots, args.json)


def add_common_args(parser):
    """Add common arguments to a parser."""
    parser.add_argument("--json", "-j", action="store_true", help="Output as JSON")
    parser.add_argument(
        "--account",
        "-a",
        default=DEFAULT_ACCOUNT,
        help=f"Email account (default: {DEFAULT_ACCOUNT})",
    )


def main():
    parser = argparse.ArgumentParser(
        prog="h8",
        description="EWS CLI for calendar, mail, and contacts",
    )
    parser.add_argument(
        "--version", action="version", version=f"%(prog)s {__version__}"
    )
    add_common_args(parser)

    subparsers = parser.add_subparsers(dest="command", required=True)

    # Calendar commands
    cal_parser = subparsers.add_parser(
        "calendar", aliases=["cal"], help="Calendar operations"
    )
    add_common_args(cal_parser)
    cal_subparsers = cal_parser.add_subparsers(dest="subcommand", required=True)

    # calendar list
    cal_list = cal_subparsers.add_parser(
        "list", aliases=["ls"], help="List calendar events"
    )
    add_common_args(cal_list)
    cal_list.add_argument(
        "--days", "-d", type=int, default=7, help="Number of days to show"
    )
    cal_list.add_argument("--from", dest="from_date", help="Start date (ISO format)")
    cal_list.add_argument("--to", dest="to_date", help="End date (ISO format)")
    cal_list.set_defaults(func=cmd_calendar_list)

    # calendar create
    cal_create = cal_subparsers.add_parser(
        "create", aliases=["new"], help="Create event (JSON from stdin)"
    )
    add_common_args(cal_create)
    cal_create.add_argument(
        "--extracted",
        "-e",
        action="store_true",
        help="Input is in extraction/event.json schema format (from xtr)",
    )
    cal_create.set_defaults(func=cmd_calendar_create)

    # calendar add (human-friendly)
    cal_add = cal_subparsers.add_parser(
        "add",
        help="Add event with natural language (e.g., 'friday 2pm \"Meeting\" with alice')",
    )
    add_common_args(cal_add)
    cal_add.add_argument(
        "input",
        nargs="+",
        help='Natural language event description (e.g., friday 2pm "Team Sync" with roman)',
    )
    cal_add.add_argument(
        "--duration",
        "-d",
        type=int,
        default=60,
        help="Default duration in minutes if not specified (default: 60)",
    )
    cal_add.add_argument(
        "--location",
        "-l",
        help="Event location",
    )
    cal_add.set_defaults(func=cmd_calendar_add)

    # calendar delete
    cal_delete = cal_subparsers.add_parser(
        "delete", aliases=["rm"], help="Delete event"
    )
    add_common_args(cal_delete)
    cal_delete.add_argument("--id", required=True, help="Event ID")
    cal_delete.set_defaults(func=cmd_calendar_delete)

    # Mail commands
    mail_parser = subparsers.add_parser("mail", aliases=["m"], help="Mail operations")
    add_common_args(mail_parser)
    mail_subparsers = mail_parser.add_subparsers(dest="subcommand", required=True)

    # mail list
    mail_list = mail_subparsers.add_parser("list", aliases=["ls"], help="List messages")
    add_common_args(mail_list)
    mail_list.add_argument("--folder", "-f", default="inbox", help="Folder name")
    mail_list.add_argument("--limit", "-l", type=int, default=20, help="Max messages")
    mail_list.add_argument("--unread", "-u", action="store_true", help="Only unread")
    mail_list.set_defaults(func=cmd_mail_list)

    # mail get
    mail_get = mail_subparsers.add_parser("get", help="Get full message")
    add_common_args(mail_get)
    mail_get.add_argument("--id", required=True, help="Message ID")
    mail_get.add_argument("--folder", "-f", default="inbox", help="Folder name")
    mail_get.set_defaults(func=cmd_mail_get)

    # mail fetch
    mail_fetch = mail_subparsers.add_parser("fetch", help="Fetch to maildir/mbox")
    add_common_args(mail_fetch)
    mail_fetch.add_argument("--folder", "-f", default="inbox", help="Folder name")
    mail_fetch.add_argument("--output", "-o", required=True, help="Output directory")
    mail_fetch.add_argument("--format", choices=["maildir", "mbox"], default="maildir")
    mail_fetch.add_argument("--limit", "-l", type=int, help="Max messages")
    mail_fetch.set_defaults(func=cmd_mail_fetch)

    # mail send
    mail_send = mail_subparsers.add_parser("send", help="Send email (JSON from stdin)")
    add_common_args(mail_send)
    mail_send.set_defaults(func=cmd_mail_send)

    # mail attachments
    mail_attach = mail_subparsers.add_parser(
        "attachments", aliases=["att"], help="List or download attachments"
    )
    add_common_args(mail_attach)
    mail_attach.add_argument("--id", required=True, help="Message ID")
    mail_attach.add_argument("--folder", "-f", default="inbox", help="Folder name")
    mail_attach.add_argument(
        "--download",
        "-d",
        type=int,
        metavar="INDEX",
        help="Download attachment by index",
    )
    mail_attach.add_argument("--output", "-o", help="Output path (directory or file)")
    mail_attach.set_defaults(func=cmd_mail_attachments)

    # Contacts commands
    contacts_parser = subparsers.add_parser(
        "contacts", aliases=["c"], help="Contacts operations"
    )
    add_common_args(contacts_parser)
    contacts_subparsers = contacts_parser.add_subparsers(
        dest="subcommand", required=True
    )

    # contacts list
    contacts_list = contacts_subparsers.add_parser(
        "list", aliases=["ls"], help="List contacts"
    )
    add_common_args(contacts_list)
    contacts_list.add_argument(
        "--limit", "-l", type=int, default=100, help="Max contacts"
    )
    contacts_list.add_argument("--search", "-s", help="Search by name or email")
    contacts_list.set_defaults(func=cmd_contacts_list)

    # contacts get
    contacts_get = contacts_subparsers.add_parser("get", help="Get contact")
    add_common_args(contacts_get)
    contacts_get.add_argument("--id", required=True, help="Contact ID")
    contacts_get.set_defaults(func=cmd_contacts_get)

    # contacts create
    contacts_create = contacts_subparsers.add_parser(
        "create", aliases=["new"], help="Create contact (JSON from stdin)"
    )
    add_common_args(contacts_create)
    contacts_create.add_argument(
        "--extracted",
        "-e",
        action="store_true",
        help="Input is in extraction/contact_details.json schema format (from xtr)",
    )
    contacts_create.set_defaults(func=cmd_contacts_create)

    # contacts delete
    contacts_delete = contacts_subparsers.add_parser(
        "delete", aliases=["rm"], help="Delete contact"
    )
    add_common_args(contacts_delete)
    contacts_delete.add_argument("--id", required=True, help="Contact ID")
    contacts_delete.set_defaults(func=cmd_contacts_delete)

    # Free slots command
    free_parser = subparsers.add_parser("free", help="Find free slots in calendar")
    add_common_args(free_parser)
    free_parser.add_argument(
        "--weeks",
        "-w",
        type=int,
        default=1,
        help="Number of weeks to look at (1 = current week)",
    )
    free_parser.add_argument(
        "--duration",
        "-d",
        type=int,
        default=30,
        help="Minimum slot duration in minutes",
    )
    free_parser.add_argument(
        "--limit", "-l", type=int, help="Maximum number of slots to return"
    )
    free_parser.set_defaults(func=cmd_free)

    # People commands (view other people's calendars)
    ppl_parser = subparsers.add_parser(
        "ppl", aliases=["people"], help="Other people's calendar operations"
    )
    add_common_args(ppl_parser)
    ppl_subparsers = ppl_parser.add_subparsers(dest="subcommand", required=True)

    # ppl agenda - view another person's calendar events
    ppl_agenda = ppl_subparsers.add_parser(
        "agenda", help="View another person's calendar events"
    )
    add_common_args(ppl_agenda)
    ppl_agenda.add_argument("person", help="Person alias or email address")
    ppl_agenda.add_argument(
        "--days", "-d", type=int, default=7, help="Number of days to show"
    )
    ppl_agenda.add_argument("--from", dest="from_date", help="Start date (ISO format)")
    ppl_agenda.add_argument("--to", dest="to_date", help="End date (ISO format)")
    ppl_agenda.set_defaults(func=cmd_ppl_agenda)

    # ppl free - find free slots in another person's calendar
    ppl_free = ppl_subparsers.add_parser(
        "free", help="Find free slots in another person's calendar"
    )
    add_common_args(ppl_free)
    ppl_free.add_argument("person", help="Person alias or email address")
    ppl_free.add_argument(
        "--weeks",
        "-w",
        type=int,
        default=1,
        help="Number of weeks to look at (1 = current week)",
    )
    ppl_free.add_argument(
        "--duration",
        "-d",
        type=int,
        default=30,
        help="Minimum slot duration in minutes",
    )
    ppl_free.add_argument(
        "--limit", "-l", type=int, help="Maximum number of slots to return"
    )
    ppl_free.set_defaults(func=cmd_ppl_free)

    # ppl common - find common free slots between multiple people
    ppl_common = ppl_subparsers.add_parser(
        "common", help="Find common free slots between multiple people"
    )
    add_common_args(ppl_common)
    ppl_common.add_argument(
        "people", nargs="+", help="Person aliases or email addresses (2 or more)"
    )
    ppl_common.add_argument(
        "--weeks",
        "-w",
        type=int,
        default=1,
        help="Number of weeks to look at (1 = current week)",
    )
    ppl_common.add_argument(
        "--duration",
        "-d",
        type=int,
        default=30,
        help="Minimum slot duration in minutes",
    )
    ppl_common.add_argument(
        "--limit", "-l", type=int, help="Maximum number of slots to return"
    )
    ppl_common.set_defaults(func=cmd_ppl_common)

    args = parser.parse_args()

    try:
        args.func(args)
    except Exception as e:
        output({"error": str(e)}, args.json)
        sys.exit(1)


if __name__ == "__main__":
    main()
