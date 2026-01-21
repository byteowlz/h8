# h8 Command Reference

Complete reference for h8 CLI commands.

## Main Commands

```
h8 [OPTIONS] <COMMAND>

Commands:
  calendar     Calendar operations
  mail         Email operations
  agenda       Today's agenda (calendar)
  contacts     Contact management
  free         Find free time slots
  ppl          Other people's calendar operations
  config       Configuration management
  init         Initialize h8
  service      Manage the Python EWS service
  completions  Generate shell completions
```

## Global Options

```
--config <PATH>      Configuration file path
-q, --quiet          Suppress output
-v, --verbose        Verbose output (can be repeated: -vv, -vvv)
--debug              Debug output
--trace              Trace output
--json               JSON output format
--yaml               YAML output format
--no-color           Disable colored output
--color <COLOR>      Color mode: auto, always, never
--dry-run            Dry run mode
-y, --yes            Skip confirmations
--timeout <SECONDS>  Command timeout
--no-progress        Disable progress indicators
--diagnostics        Show diagnostic information
-a, --account <ACCOUNT>  Use specific account
```

---

## Mail Commands

### mail list

List messages in a folder.

```bash
h8 mail list [OPTIONS]
```

**Output**: List of recent emails with subject, sender, and timestamp.

### mail send

Send an email.

```bash
h8 mail send [OPTIONS] [ID]

Arguments:
  [ID]  Draft ID to send (optional)

Options:
  --file <FILE>          Read email from file
  --all                  Send all drafts
  -s, --schedule <SCHEDULE>  Schedule delivery (e.g., "tomorrow 9am", "friday 14:00")
  -t, --to <TO>          Recipient email (can specify multiple times)
  -c, --cc <CC>          CC recipient (can specify multiple times)
  --bcc <BCC>            BCC recipient (can specify multiple times)
  --subject <SUBJECT>    Email subject
  -b, --body <BODY>      Email body (use "-" for stdin)
  --html                 Treat body as HTML
```

**Examples**:
```bash
# Simple email
h8 mail send --to alice@example.com --subject "Hello" --body "Hi Alice!"

# Multiple recipients with CC
h8 mail send --to alice@example.com --to bob@example.com --cc manager@example.com --subject "Team Update" --body "Status report..."

# Scheduled email
h8 mail send --to alice@example.com --subject "Reminder" --body "Meeting tomorrow" --schedule "tomorrow 9am"

# HTML email
h8 mail send --to alice@example.com --subject "Newsletter" --body "<h1>News</h1><p>Updates...</p>" --html
```

### mail search

Search messages by subject, sender, or body.

```bash
h8 mail search <query>
```

**Examples**:
```bash
h8 mail search "project update"
h8 mail search "alice@example.com"
```

### mail read

Read a message (view in pager).

```bash
h8 mail read <message-id>
```

### mail reply

Reply to a message.

```bash
h8 mail reply <message-id>
```

### mail forward

Forward a message.

```bash
h8 mail forward <message-id>
```

### mail compose

Compose a new email (opens editor).

```bash
h8 mail compose [OPTIONS]

Options:
  --no-edit  Open editor immediately
```

### mail mark

Mark a message (read/unread/flagged).

```bash
h8 mail mark <message-id> <status>
```

### mail delete

Delete a message (move to trash).

```bash
h8 mail delete <message-id>
```

### mail attachments

List or download attachments.

```bash
h8 mail attachments [OPTIONS] <message-id>
```

---

## Calendar Commands

### calendar add

Add event with natural language.

```bash
h8 calendar add [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...  Natural language event description

Options:
  -d, --duration <DURATION>  Default duration in minutes [default: 60]
  -l, --location <LOCATION>  Event location
```

**Natural Language Examples**:
```bash
# Time-based
h8 calendar add "friday 2pm Team Sync"
h8 calendar add "tomorrow 14:00 Project Review"
h8 calendar add "next monday 10am Sprint Planning"

# With duration
h8 calendar add "friday 2pm-4pm Workshop"
h8 calendar add "tomorrow 9am Team Standup" --duration 15

# With location
h8 calendar add "friday 2pm Team Sync" --location "Conference Room A"
h8 calendar add "tomorrow 3pm Client Call" --location "Zoom"

# With attendees (in description)
h8 calendar add "friday 2pm Meeting with alice@example.com"
h8 calendar add "tomorrow 10am Sync with bob and charlie"
```

### calendar show

Show events with natural language dates.

```bash
h8 calendar show <date-expression>
```

**Examples**:
```bash
h8 calendar show "today"
h8 calendar show "tomorrow"
h8 calendar show "next week"
h8 calendar show "friday"
h8 calendar show "kw30"  # Calendar week 30
h8 calendar show "january"
```

### calendar list

List calendar events.

```bash
h8 calendar list [OPTIONS]
```

### calendar search

Search events by subject, location, or body.

```bash
h8 calendar search <query>
```

### calendar delete

Delete a calendar event.

```bash
h8 calendar delete <event-id>
```

### calendar invite

Send meeting invite to attendees.

```bash
h8 calendar invite [OPTIONS] --subject <SUBJECT> --start <START> --end <END>

Options:
  -s, --subject <SUBJECT>    Meeting subject
  --start <START>            Start time (ISO format, e.g., 2026-01-22T14:00:00)
  --end <END>                End time (ISO format)
  -t, --to <TO>              Required attendee email (can specify multiple times)
  -o, --optional <OPTIONAL>  Optional attendee email (can specify multiple times)
  -l, --location <LOCATION>  Meeting location
  -b, --body <BODY>          Meeting body/description
```

**Examples**:
```bash
# Simple meeting invite
h8 calendar invite --subject "Team Sync" \
  --start 2026-01-22T14:00:00 --end 2026-01-22T15:00:00 \
  --to alice@example.com

# Multiple attendees with optional
h8 calendar invite --subject "Sprint Planning" \
  --start 2026-01-23T10:00:00 --end 2026-01-23T12:00:00 \
  --to alice@example.com --to bob@example.com \
  --optional charlie@example.com \
  --location "Conference Room A" \
  --body "Quarterly sprint planning session"
```

### calendar invites

List pending meeting invites from inbox.

```bash
h8 calendar invites [OPTIONS]

Options:
  -n, --limit <LIMIT>  Maximum results to return [default: 50]
```

**Examples**:
```bash
# List all pending invites
h8 calendar invites

# JSON output for processing
h8 calendar invites --json
```

### calendar rsvp

Respond to a meeting invite (accept/decline/tentative).

```bash
h8 calendar rsvp [OPTIONS] <ID> <RESPONSE>

Arguments:
  <ID>        Meeting invite ID
  <RESPONSE>  Response: accept, decline, tentative, maybe

Options:
  -m, --message <MESSAGE>  Optional message to include with response
```

**Examples**:
```bash
# Accept an invite
h8 calendar rsvp abc123 accept

# Decline with message
h8 calendar rsvp abc123 decline --message "I have a conflict at this time"

# Tentatively accept
h8 calendar rsvp abc123 tentative --message "Need to check with my team first"
```

---

## Agenda Command

View today's agenda.

```bash
h8 agenda [OPTIONS]
```

**Output**: List of today's events with time, title, and location.

---

## People (ppl) Commands

### ppl free

Find free slots in another person's calendar.

```bash
h8 ppl free [OPTIONS] <PERSON>

Arguments:
  <PERSON>  Person alias or email address

Options:
  -w, --weeks <WEEKS>        Number of weeks to check [default: 1]
  -d, --duration <DURATION>  Minimum slot duration in minutes [default: 30]
  -l, --limit <LIMIT>        Limit number of results
  -V, --view <VIEW>          View mode: list, gantt, compact
```

**Examples**:
```bash
# Check availability for next week
h8 ppl free alice@example.com

# Find 2-hour slots over 2 weeks
h8 ppl free alice@example.com --weeks 2 --duration 120

# Compact view
h8 ppl free alice@example.com --view compact
```

### ppl agenda

View another person's calendar events.

```bash
h8 ppl agenda <PERSON>
```

### ppl common

Find common free slots between multiple people.

```bash
h8 ppl common <email1> <email2> [email3...]
```

**Examples**:
```bash
# Two people
h8 ppl common alice@example.com bob@example.com

# Multiple people
h8 ppl common alice@example.com bob@example.com charlie@example.com manager@example.com
```

---

## Service Commands

### service start

Start the h8 Python EWS service.

```bash
h8 service start
```

### service stop

Stop the h8 Python EWS service.

```bash
h8 service stop
```

### service restart

Restart the h8 Python EWS service.

```bash
h8 service restart
```

### service status

Check h8 service status.

```bash
h8 service status
```

---

## Configuration Commands

### config

View or edit configuration.

```bash
h8 config [OPTIONS]
```

### init

Initialize h8 (first-time setup).

```bash
h8 init
```

---

## Output Formats

h8 supports multiple output formats:

### Human-Readable (Default)

```bash
h8 mail list
```

### JSON

```bash
h8 mail list --json
```

### YAML

```bash
h8 mail list --yaml
```

---

## Tips and Tricks

### Chaining Commands

```bash
# Check agenda then send summary
h8 agenda --json | jq -r '.[] | .subject' | xargs -I {} h8 mail send --to manager@example.com --subject "My Schedule" --body "Events: {}"
```

### Using Aliases

Create shell aliases for common operations:

```bash
alias check-mail='h8 mail list'
alias today='h8 agenda'
alias send-mail='h8 mail send'
```

### Scheduling Emails

```bash
# Send email tomorrow morning
h8 mail send --to team@example.com --subject "Daily Standup" --body "Meeting at 9am" --schedule "tomorrow 8am"

# Send on specific date
h8 mail send --to alice@example.com --subject "Reminder" --body "Project deadline" --schedule "2026-01-25 14:00"
```

### Natural Language Parsing

h8 calendar add supports flexible natural language:

- **Days**: today, tomorrow, monday, next friday, etc.
- **Times**: 2pm, 14:00, 9am, noon, midnight
- **Durations**: 2pm-4pm, 30min, 2h, etc.
- **Relative**: in 2 hours, in 3 days, etc.

---

## Error Handling

### Service Not Running

```bash
# Check status
h8 service status

# Start if needed
h8 service start
```

### Authentication Issues

```bash
# Re-initialize
h8 init

# Check configuration
h8 config
```

### Invalid Email/Calendar Input

- Ensure email addresses are properly formatted
- Use clear natural language for calendar entries
- Specify times explicitly when ambiguous
