---
name: microsoft-exchange-agent
description: |
  Automate Microsoft Exchange email and calendar operations using h8 CLI (Rust CLI for Exchange Web Services).
  This skill should be used when users ask to check emails, manage calendar events, send emails,
  check availability, schedule meetings, or perform any Microsoft Exchange/EWS-related tasks.
allowed-tools: Bash
---

# h8 CLI - Exchange Email/Calendar

## Email

```bash
h8 mail list [today|monday|"jan 15"] [-u unread] [-f folder] [-l limit]
h8 mail read <id>
h8 mail search "from:alice" | "subject:meeting"
h8 mail send --to a@x.com --subject "Hi" --body "text"
h8 mail reply <id> [--all]
h8 mail move <id> --to <folder>
h8 mail move -q "from:newsletter" --to newsletters   # search+move bulk
h8 mail delete <id> [--force]
h8 mail sync                      # sync inbox, populates address cache
```

## Calendar

```bash
h8 cal show [today|tomorrow|friday|"next week"|kw30]
h8 cal add 'friday 2pm Team Sync'
h8 cal add 'tomorrow 10am Meeting with alice@x.com'  # sends invite
h8 cal search "standup"
h8 cal cancel <id>                # cancel + notify attendees
h8 cal cancel -q today            # cancel all today's meetings
h8 cal cancel -q today --dry-run  # preview what would be cancelled
h8 cal invites                    # pending invites
h8 cal rsvp <id> accept|decline
```

## Availability

```bash
h8 free [-w weeks] [-d duration_mins]
h8 ppl free <person>              # their free slots
h8 ppl common <person1> <person2> # common free time
```

## Contacts & Addresses

```bash
h8 contacts list [-s search]
h8 contacts update --id <id> --phone|--email|--name|--company <value>

h8 addr                           # frequent addresses (from sent/received)
h8 addr "alice"                   # search cached addresses
h8 addr --frequent                # most used addresses
```

## Key Patterns

```bash
# Schedule meeting with availability check
h8 ppl free alice
h8 cal add 'friday 2pm Sync with alice@x.com'

# Bulk organize emails
h8 mail move -q "from:notifications@" --to notifications

# Cancel all meetings today
h8 cal cancel -q today -m "Out sick"

# Find email address
h8 addr "horst"

# JSON output for parsing
h8 mail list --json
```

## Notes

- IDs: word-based like `cold-lamp` (mail/calendar) or Exchange IDs (contacts)
- Natural dates: today, tomorrow, weekdays, "next week", kw30, "jan 15"
- `with <email>` in cal add auto-sends meeting invite
- Folders auto-created on move
- Address cache populated by `h8 mail sync`
