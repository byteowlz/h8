---
name: microsoft-exchange-agent
description: |
  Automate Microsoft Exchange email and calendar operations using h8 CLI (Rust CLI for Exchange Web Services).
  This skill should be used when users ask to check emails, manage calendar events, send emails,
  check availability, schedule meetings, or perform any Microsoft Exchange/EWS-related tasks.
allowed-tools: Bash
---

# Microsoft Exchange Agent Skill

Automate email and calendar operations using the h8 CLI tool - a Rust-based CLI for Exchange Web Services (EWS) backed by a Python service.

## What This Skill Does

- Check and manage emails (list, read, send, search, reply, forward)
- Manage calendar events (view agenda, create events, check availability)
- Check other people's availability and find common meeting times
- Send meeting invites with attendees and RSVP to received invites
- Process email attachments
- Search and filter emails and calendar events

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
```

**Search Emails**
```bash
h8 mail search <query>
```

**Read Email**
```bash
h8 mail read <message-id>
```

**Send Email**
```bash
h8 mail send --to <recipient> --subject <subject> --body <body>
```

**Reply to Email**
```bash
h8 mail reply <message-id>
```

### Calendar Operations

**View Today's Agenda**
```bash
h8 agenda
```

**Add Calendar Event** (uses natural language)
```bash
h8 calendar add "friday 2pm Meeting with alice"
```

**Show Calendar Events**
```bash
h8 calendar show "next week"
h8 calendar show "kw30"
```

**Search Calendar**
```bash
h8 calendar search <query>
```

### Meeting Invites & RSVP

**Send Meeting Invite**
```bash
h8 calendar invite --subject "Meeting" --start 2026-01-22T14:00:00 --end 2026-01-22T15:00:00 --to attendee@example.com
```

**List Pending Invites**
```bash
h8 calendar invites
```

**Respond to Invite**
```bash
h8 calendar rsvp <invite-id> accept
h8 calendar rsvp <invite-id> decline --message "I have a conflict"
h8 calendar rsvp <invite-id> tentative
```

### Availability & Scheduling

**Check Someone's Availability**
```bash
h8 ppl free <email-address>
```

**View Someone's Agenda**
```bash
h8 ppl agenda <email-address>
```

**Find Common Free Slots**
```bash
h8 ppl common <email1> <email2> [email3...]
```

---

## Workflows

### Check Emails Workflow

1. List recent emails: `h8 mail list`
2. If specific email needed, read it: `h8 mail read <id>`
3. Optionally reply, forward, or mark as read

### Schedule Meeting Workflow

1. Check your agenda: `h8 agenda`
2. Check attendee availability: `h8 ppl free <email>`
3. Send meeting invite: `h8 calendar invite --subject "Meeting" --start <datetime> --end <datetime> --to <email>`

### RSVP to Meeting Workflow

1. List pending invites: `h8 calendar invites`
2. Review invite details
3. Respond: `h8 calendar rsvp <invite-id> accept` (or decline/tentative)

### Send Email with Context Workflow

1. Optionally search for related emails: `h8 mail search <topic>`
2. Compose and send: `h8 mail send --to <recipient> --subject <subject> --body <message>`

---

## Best Practices

### Email Management

- **List before reading**: Always list emails first to see what's available
- **Use search**: For specific topics, use `h8 mail search` instead of listing all
- **Natural language subjects**: Keep subjects clear and concise
- **Professional tone**: Maintain appropriate email etiquette

### Calendar Management

- **Natural language**: h8 calendar add supports natural language (e.g., "tomorrow 3pm", "friday 14:00")
- **Check conflicts**: Use `h8 agenda` before scheduling new events
- **Include location**: Specify location when creating events
- **Time zones**: Be mindful of time zone differences when scheduling

### Availability Checking

- **Check first**: Always check availability before sending meeting invitations
- **Find common slots**: Use `h8 ppl common` for multiple attendees
- **Consider time zones**: Account for different time zones when working with remote teams

---

## Common Patterns

### Pattern 1: Quick Email Check
```bash
# List recent emails
h8 mail list

# Read specific email if needed
h8 mail read <message-id>
```

### Pattern 2: Schedule Meeting with Availability Check
```bash
# Check your schedule
h8 agenda

# Check attendee availability
h8 ppl free attendee@example.com

# Send meeting invite (creates event and sends invite)
h8 calendar invite --subject "Team Sync" \
  --start 2026-01-22T14:00:00 --end 2026-01-22T15:00:00 \
  --to attendee@example.com \
  --location "Conference Room A"
```

### Pattern 2b: RSVP to Meeting Invites
```bash
# List pending invites
h8 calendar invites

# Accept an invite
h8 calendar rsvp <invite-id> accept

# Decline with message
h8 calendar rsvp <invite-id> decline --message "I have a conflict at this time"

# Tentatively accept
h8 calendar rsvp <invite-id> tentative
```

### Pattern 3: Find Common Meeting Time
```bash
# Find common free slots
h8 ppl common person1@example.com person2@example.com person3@example.com

# Create event at identified time
h8 calendar add "friday 3pm Planning Meeting" --location "Zoom"
```

### Pattern 4: Email Search and Reply
```bash
# Search for emails on topic
h8 mail search "project update"

# Read specific result
h8 mail read <message-id>

# Reply
h8 mail reply <message-id>
```

---

## Error Handling

### Common Issues

**Service not running**
- Check if h8 service is running: `h8 service status`
- Start if needed: `h8 service start`

**Authentication errors**
- Verify h8 is properly initialized: `h8 config`
- Re-authenticate if necessary

**Invalid email format**
- Ensure email addresses are properly formatted
- Use full email addresses (user@domain.com)

**Calendar parsing errors**
- Use clear natural language
- Specify time explicitly (e.g., "2pm" not "afternoon")
- Include date when not "today" or named day

---

## Output Processing

h8 supports multiple output formats:

- **Default**: Human-readable text
- **JSON**: `--json` flag for programmatic parsing
- **YAML**: `--yaml` flag for structured output

Example:
```bash
# Get JSON output for parsing
h8 mail list --json

# Get YAML output
h8 agenda --yaml
```

---

## Implementation Checklist

Before executing h8 commands:

- [ ] h8 is installed and accessible
- [ ] User's specific request is clear
- [ ] Required parameters gathered (email addresses, subjects, times, etc.)
- [ ] Service is running (if needed)
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
- `workflows.md`: Detailed workflow patterns
