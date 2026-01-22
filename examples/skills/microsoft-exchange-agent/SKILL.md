---
name: microsoft-exchange-agent
description: |
  Automate Microsoft Exchange email and calendar operations using h8 CLI (Rust CLI for Exchange Web Services).
  This skill should be used when users ask to check emails, manage calendar events, send emails,
  check availability, schedule meetings, or perform any Microsoft Exchange/EWS-related tasks.
allowed-tools: Bash
---

# h8 Exchange Agent

CLI tool for Microsoft Exchange email and calendar operations.

## Email

```bash
# List emails
h8 mail list                      # inbox, last 20
h8 mail list -f sent -l 50        # sent folder, 50 items
h8 mail list --unread             # unread only
h8 mail list today                # today's emails
h8 mail list yesterday            # yesterday's emails

# Read email
h8 mail get --id <id>

# Search
h8 mail search "query"            # search inbox
h8 mail search "from:alice"       # by sender

# Send (JSON via stdin)
echo '{"to":["a@x.com"],"subject":"Hi","body":"Hello"}' | h8 mail send
echo '{"to":["a@x.com"],"cc":["b@x.com"],"subject":"Update","body":"..."}' | h8 mail send
echo '{"to":["a@x.com"],"subject":"Later","body":"...","schedule_at":"2026-01-22T09:00:00"}' | h8 mail send

# Attachments
h8 mail attachments <id>                    # list
h8 mail attachments <id> -d 0 -o ./out/     # download first

# Delete/Move
h8 mail delete <id>               # move to trash
h8 mail delete <id> --force       # permanent delete
h8 mail move <id> --to archive    # move to folder
h8 mail spam <id>                 # mark as spam
h8 mail spam <id> --not-spam      # mark as not spam
h8 mail empty-folder trash -y     # empty trash
```

## Calendar

```bash
# View schedule
h8 calendar show today
h8 calendar show tomorrow
h8 calendar show friday
h8 calendar show "next week"
h8 calendar list --days 14

# Search
h8 calendar search "standup"

# Add event (natural language)
h8 calendar add friday 2pm Team Sync
h8 calendar add 'tomorrow 10am Meeting with alice@example.com'
h8 calendar add 'friday 2pm-4pm Workshop' --location "Room A"
h8 calendar add 'monday 9am Standup' --duration 15

# Delete
h8 calendar delete <id>

# Meeting invites
h8 calendar invite --subject "Sync" --start 2026-01-22T14:00:00 --end 2026-01-22T15:00:00 --attendees alice@x.com
h8 calendar invites                # list pending invites
h8 calendar rsvp <id> --accept     # accept invite
h8 calendar rsvp <id> --decline    # decline invite
```

## Availability

```bash
# Your free slots
h8 free
h8 free --weeks 2 --duration 60

# Someone else's availability
h8 ppl free alice@example.com
h8 ppl agenda alice@example.com

# Common free time (2+ people)
h8 ppl common alice@x.com bob@x.com
```

## Contacts

```bash
h8 contacts list
h8 contacts list -s "alice"       # search
h8 contacts get --id <id>
```

## Key Patterns

**Check then act:**

```bash
h8 calendar show today            # check schedule
h8 ppl free alice@example.com     # check availability
h8 calendar add 'friday 2pm Sync with alice@example.com'
```

**Send email:**

```bash
cat <<'EOF' | h8 mail send
{"to":["team@x.com"],"subject":"Update","body":"Status report..."}
EOF
```

**JSON output for parsing:**

```bash
h8 mail list --json
h8 calendar show today --json
```

## Notes

- IDs can be word-based (e.g., `tiger-castle`) or Exchange IDs
- Use `--json` flag for programmatic output
- Natural language supports: today, tomorrow, weekdays, "next monday", dates
- `with email@x.com` in calendar add sends meeting invite automatically
