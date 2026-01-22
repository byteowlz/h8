---
name: microsoft-exchange-agent
description: |
  Automate Microsoft Exchange email and calendar operations using h8 CLI (Rust CLI for Exchange Web Services).
  This skill should be used when users ask to check emails, manage calendar events, send emails,
  check availability, schedule meetings, or perform any Microsoft Exchange/EWS-related tasks.
allowed-tools: Bash
---

# Microsoft Exchange Agent Skill

Automate email and calendar operations using the h8 CLI tool - a Python CLI for Exchange Web Services (EWS).

## What This Skill Does

- Check and manage emails (list, read, send)
- Manage calendar events (view schedule, create events with attendees)
- Check other people's availability and find common meeting times
- Process email attachments

## What This Skill Does NOT Do

- Configure h8 initial setup (use `h8 init` manually)
- Manage h8 service lifecycle (start/stop/restart)
- Modify h8 configuration files directly
- Handle authentication or account setup

---

## Before Implementation

Gather context to ensure successful implementation:

| Source | Gather |
|--------|--------|
| **Conversation** | User's specific request (check emails, send message, schedule meeting, etc.) |
| **Skill References** | h8 command patterns, common workflows, best practices |
| **User Guidelines** | Email etiquette, calendar preferences, organizational standards |

Ensure all required context is gathered before executing h8 commands.

---

## Core Operations

### Email Operations

**List Emails**
```bash
h8 mail list
h8 mail list --folder sent
h8 mail list --unread
h8 mail list --limit 50
```

**Get Full Email by ID**
```bash
h8 mail get --id <message-id>
```

**Send Email** (requires JSON via stdin)
```bash
# Simple email
echo '{"to": ["alice@example.com"], "subject": "Hello", "body": "Hi Alice!"}' | h8 mail send

# Multiple recipients with CC
echo '{"to": ["alice@example.com", "bob@example.com"], "cc": ["manager@example.com"], "subject": "Team Update", "body": "Status report..."}' | h8 mail send

# HTML email
echo '{"to": ["alice@example.com"], "subject": "Newsletter", "body": "<h1>News</h1><p>Updates...</p>", "html": true}' | h8 mail send

# Scheduled email (sends later)
echo '{"to": ["alice@example.com"], "subject": "Reminder", "body": "Meeting tomorrow", "schedule_at": "2026-01-22T09:00:00"}' | h8 mail send
```

**List/Download Attachments**
```bash
h8 mail attachments --id <message-id>
h8 mail attachments --id <message-id> --download 0 --output ./downloads/
```

### Calendar Operations

**View Calendar Events**
```bash
# Today's events
h8 calendar show today

# Tomorrow
h8 calendar show tomorrow

# Specific day
h8 calendar show friday
h8 calendar show "next monday"

# Date range
h8 calendar show "next week"
h8 calendar show "kw30"  # Calendar week 30
h8 calendar show january
```

**List Calendar Events (raw)**
```bash
h8 calendar list
h8 calendar list --days 14
h8 calendar list --from 2026-01-20 --to 2026-01-25
```

**Add Calendar Event** (uses natural language with attendees)
```bash
# Simple event
h8 calendar add friday 2pm Team Sync

# With explicit title (use quotes)
h8 calendar add 'friday 2pm "Sprint Planning"'

# With attendee (use "with" keyword)
h8 calendar add 'friday 2pm Meeting with alice@example.com'
h8 calendar add 'tomorrow 10am Sync with bob@example.com'

# With duration
h8 calendar add 'friday 2pm-4pm Workshop'
h8 calendar add 'tomorrow 9am Team Standup' --duration 15

# With location
h8 calendar add 'friday 2pm Team Sync' --location "Conference Room A"
h8 calendar add 'tomorrow 3pm Client Call' --location "Zoom"
```

**Create Event from JSON** (for complex events)
```bash
echo '{"subject": "Team Meeting", "start": "2026-01-22T14:00:00", "end": "2026-01-22T15:00:00", "location": "Room A"}' | h8 calendar create
```

**Delete Event**
```bash
h8 calendar delete --id <event-id>
```

### Availability & Scheduling

**Check Someone's Availability**
```bash
h8 ppl free alice@example.com
h8 ppl free alice@example.com --weeks 2
h8 ppl free alice@example.com --duration 120  # 2-hour slots
```

**View Someone's Agenda**
```bash
h8 ppl agenda alice@example.com
h8 ppl agenda alice@example.com --days 14
```

**Find Common Free Slots**
```bash
h8 ppl common alice@example.com bob@example.com
h8 ppl common alice@example.com bob@example.com charlie@example.com --weeks 2
```

**Find Your Own Free Slots**
```bash
h8 free
h8 free --weeks 2 --duration 60
```

---

## Workflows

### Check Emails Workflow

1. List recent emails: `h8 mail list`
2. If specific email needed, read it: `h8 mail get --id <id>`

### Schedule Meeting with Attendee Workflow

1. Check your schedule: `h8 calendar show today`
2. Check attendee availability: `h8 ppl free alice@example.com`
3. Create meeting with attendee: `h8 calendar add 'friday 2pm Meeting with alice@example.com'`

### Send Email Workflow

1. Compose JSON with to, subject, body
2. Send via stdin: `echo '{"to": ["recipient@example.com"], "subject": "Subject", "body": "Message"}' | h8 mail send`

---

## Best Practices

### Email Management

- **List before reading**: Always list emails first to see what's available
- **JSON for send**: Always use JSON format piped via stdin for sending emails
- **Escape quotes**: Be careful with JSON quoting in bash

### Calendar Management

- **Natural language**: `h8 calendar add` supports natural language (e.g., "tomorrow 3pm", "friday 14:00")
- **Use "with" for attendees**: Include attendees using `with alice@example.com` in the natural language
- **Check conflicts**: Use `h8 calendar show` before scheduling new events
- **Include location**: Specify location with `--location` when creating events

### Availability Checking

- **Check first**: Always check availability before scheduling meetings
- **Find common slots**: Use `h8 ppl common` for multiple attendees
- **Consider time zones**: Account for different time zones when working with remote teams

---

## Common Patterns

### Pattern 1: Quick Email Check
```bash
# List recent emails
h8 mail list

# Read specific email if needed
h8 mail get --id <message-id>
```

### Pattern 2: Schedule Meeting with Availability Check
```bash
# Check your schedule
h8 calendar show today

# Check attendee availability
h8 ppl free attendee@example.com

# Create meeting with attendee (sends invite automatically)
h8 calendar add 'friday 2pm "Team Sync" with attendee@example.com' --location "Conference Room A"
```

### Pattern 3: Find Common Meeting Time
```bash
# Find common free slots
h8 ppl common person1@example.com person2@example.com person3@example.com

# Create event at identified time
h8 calendar add 'friday 3pm "Planning Meeting"' --location "Zoom"
```

### Pattern 4: Send Email
```bash
# Send a simple email
echo '{"to": ["recipient@example.com"], "subject": "Project Update", "body": "Here is the latest status..."}' | h8 mail send

# Send to multiple recipients
echo '{"to": ["alice@example.com", "bob@example.com"], "cc": ["manager@example.com"], "subject": "Weekly Report", "body": "Attached is the report."}' | h8 mail send
```

---

## Error Handling

### Common Issues

**Service not running**
- h8 runs as a direct CLI, no service needed

**Authentication errors**
- Verify h8 is properly initialized: `h8 config`
- Re-authenticate if necessary

**Invalid email format**
- Ensure email addresses are properly formatted
- Use full email addresses (user@domain.com)
- For mail send, ensure JSON is valid

**Calendar parsing errors**
- Use clear natural language
- Specify time explicitly (e.g., "2pm" not "afternoon")
- Include date when not "today" or named day

---

## Output Processing

h8 supports multiple output formats:

- **Default**: Human-readable text
- **JSON**: `--json` flag for programmatic parsing

Example:
```bash
# Get JSON output for parsing
h8 mail list --json

# Get JSON calendar events
h8 calendar show today --json
```

---

## Implementation Checklist

Before executing h8 commands:

- [ ] h8 is installed and accessible
- [ ] User's specific request is clear
- [ ] Required parameters gathered (email addresses, subjects, times, etc.)
- [ ] Appropriate command selected
- [ ] Output format specified (if needed for processing)

After executing:

- [ ] Command executed successfully
- [ ] Output presented to user clearly
- [ ] Follow-up actions identified (if any)
- [ ] Errors handled appropriately

---

## Reference Files

See `references/` directory for:
- `command-reference.md`: Complete h8 command documentation
- `natural-language-examples.md`: Natural language parsing examples for calendar
