---
name: microsoft-exchange-agent
description: |
  Automate Microsoft Exchange email and calendar operations using h8 CLI (Rust CLI for Exchange Web Services).
  This skill should be used when users ask to check emails, manage calendar events, send emails,
  check availability, schedule meetings, or perform any Microsoft Exchange/EWS-related tasks.
allowed-tools: Bash
---

# h8 Exchange Agent

Rust CLI for Microsoft Exchange (EWS). Uses word-based short IDs (e.g., `cold-lamp`) for mail/calendar.

## Email

```bash
# List (supports natural language dates)
h8 mail list                      # inbox, last 20
h8 mail list today                # today's emails
h8 mail list monday               # emails from Monday
h8 mail list "jan 15"             # specific date
h8 mail list -u                   # unread only
h8 mail list -f sent -l 50        # sent folder, 50 items

# Read/view
h8 mail read <id>                 # view in pager (marks read)
h8 mail get --id <id>             # raw JSON

# Search
h8 mail search "meeting notes"
h8 mail search "from:alice"

# Compose & send
h8 mail compose                   # opens editor, saves draft
h8 mail drafts                    # list drafts
h8 mail send <draft-id>           # send a draft
h8 mail send --to alice@x.com --subject "Hi" --body "Hello"

# Reply/forward
h8 mail reply <id>                # reply to sender
h8 mail reply <id> --all          # reply all
h8 mail forward <id>

# Manage
h8 mail delete <id>               # move to trash
h8 mail delete <id> --force       # permanent
h8 mail move <id> --to archive
h8 mail mark <id> --read
h8 mail mark <id> --flag
h8 mail spam <id>
h8 mail empty-folder trash -y

# Attachments
h8 mail attachments <id>
h8 mail attachments <id> -d 0 -o ./downloads/
```

## Calendar

```bash
# View (natural language)
h8 cal show today
h8 cal show tomorrow
h8 cal show friday
h8 cal show "next week"
h8 cal show kw30                  # calendar week
h8 agenda                         # today's agenda (formatted)

# Add events (natural language)
h8 cal add friday 2pm Team Sync
h8 cal add 'tomorrow 10am-11am Review'
h8 cal add 'monday 9am Standup' -d 15        # 15 min duration
h8 cal add 'friday 2pm Meeting with alice'   # sends invite

# Search & delete
h8 cal search "standup"
h8 cal delete <id>

# Meeting invites
h8 cal invite -s "Sync" --start 2026-01-22T14:00:00 --end 2026-01-22T15:00:00 -t alice@x.com
h8 cal invites                    # list pending
h8 cal rsvp <id> accept
h8 cal rsvp <id> decline
```

## Availability

```bash
h8 free                           # your free slots
h8 free -w 2 -d 60                # 2 weeks, 60-min slots

h8 ppl agenda alice               # someone's calendar
h8 ppl free alice                 # their free slots
h8 ppl common alice bob           # common free time
```

## Contacts

```bash
h8 contacts list
h8 contacts list -s "horst"       # search
h8 contacts get --id <id>

# Update contact fields
h8 contacts update --id <id> --phone "+49 123 456"
h8 contacts update --id <id> --email "new@x.com" --name "New Name"
h8 contacts update --id <id> --company "Acme" --job-title "Engineer"

# Create (JSON)
echo '{"name":"Alice","email":"a@x.com","phone":"+1234"}' | h8 contacts create
h8 contacts delete --id <id>
```

## Patterns

**Schedule a meeting:**
```bash
h8 ppl free alice                 # check availability
h8 cal add 'friday 2pm Sync with alice'
```

**Send email programmatically:**
```bash
h8 mail send --to team@x.com --subject "Update" --body "Status: done"
```

**JSON output:**
```bash
h8 mail list --json | jq '.[0].subject'
h8 cal show today --json
```

## Notes

- Short IDs: `cold-lamp`, `weak-dams` (mail/calendar only, not contacts yet)
- Person aliases: configure in `~/.config/h8/config.toml` under `[people]`
- Natural dates: today, tomorrow, weekdays, "next week", kw30, "jan 15"
- `with <email>` in cal add sends meeting invite automatically
