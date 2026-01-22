# h8 Command Reference

Complete reference for h8 CLI commands.

## Main Commands

```
h8 [OPTIONS] <COMMAND>

Commands:
  calendar (cal)   Calendar operations
  mail (m)         Email operations
  contacts (c)     Contact management
  free             Find free time slots in your calendar
  ppl (people)     Other people's calendar operations
```

## Global Options

```
--json, -j           Output as JSON
--account, -a        Use specific account (default: configured account)
-h, --help           Show help
--version            Show version
```

---

## Mail Commands

### mail list

List messages in a folder.

```bash
h8 mail list [OPTIONS]

Options:
  --folder, -f <FOLDER>  Folder name (default: inbox)
  --limit, -l <LIMIT>    Max messages (default: 20)
  --unread, -u           Only show unread messages
```

**Examples:**
```bash
h8 mail list
h8 mail list --folder sent
h8 mail list --unread --limit 50
h8 mail list --json
```

### mail get

Get a full message by ID including body.

```bash
h8 mail get --id <MESSAGE_ID> [OPTIONS]

Options:
  --id <ID>              Message ID (required)
  --folder, -f <FOLDER>  Folder name (default: inbox)
```

**Examples:**
```bash
h8 mail get --id AAMkAGI2TG9...
h8 mail get --id AAMkAGI2TG9... --json
```

### mail send

Send an email. Reads JSON from stdin.

```bash
h8 mail send

# JSON schema:
{
  "to": ["email@example.com"],      # Required: list of recipient emails
  "subject": "Subject line",         # Required: email subject
  "body": "Email body text",         # Optional: email body
  "cc": ["cc@example.com"],          # Optional: CC recipients
  "html": true,                      # Optional: treat body as HTML
  "schedule_at": "2026-01-22T09:00:00"  # Optional: ISO datetime for delayed send
}
```

**Examples:**
```bash
# Simple email
echo '{"to": ["alice@example.com"], "subject": "Hello", "body": "Hi Alice!"}' | h8 mail send

# Multiple recipients with CC
echo '{"to": ["alice@example.com", "bob@example.com"], "cc": ["manager@example.com"], "subject": "Team Update", "body": "Status report..."}' | h8 mail send

# Scheduled email
echo '{"to": ["alice@example.com"], "subject": "Reminder", "body": "Meeting tomorrow", "schedule_at": "2026-01-22T09:00:00"}' | h8 mail send

# HTML email
echo '{"to": ["alice@example.com"], "subject": "Newsletter", "body": "<h1>News</h1><p>Updates...</p>", "html": true}' | h8 mail send

# Using heredoc for complex body (recommended for multi-line)
cat <<'EOF' | h8 mail send
{
  "to": ["alice@example.com"],
  "subject": "Project Update",
  "body": "Hi Alice,\n\nHere is the update you requested.\n\nBest regards"
}
EOF
```

### mail fetch

Fetch messages to maildir or mbox format.

```bash
h8 mail fetch --output <PATH> [OPTIONS]

Options:
  --folder, -f <FOLDER>  Folder name (default: inbox)
  --output, -o <PATH>    Output directory (required)
  --format <FORMAT>      Output format: maildir, mbox (default: maildir)
  --limit, -l <LIMIT>    Max messages
```

### mail attachments

List or download attachments from a message.

```bash
h8 mail attachments --id <MESSAGE_ID> [OPTIONS]

Options:
  --id <ID>              Message ID (required)
  --folder, -f <FOLDER>  Folder name (default: inbox)
  --download, -d <INDEX> Download attachment by index
  --output, -o <PATH>    Output path for download
```

**Examples:**
```bash
# List attachments
h8 mail attachments --id AAMkAGI2TG9...

# Download first attachment
h8 mail attachments --id AAMkAGI2TG9... --download 0 --output ./downloads/
```

---

## Calendar Commands

### calendar show

Show events using natural language date expressions.

```bash
h8 calendar show [WHEN]

Arguments:
  WHEN  Date expression (default: today)
```

**Examples:**
```bash
h8 calendar show today
h8 calendar show tomorrow
h8 calendar show friday
h8 calendar show "next monday"
h8 calendar show "next week"
h8 calendar show "kw30"           # Calendar week 30
h8 calendar show january
h8 calendar show "11 december"
```

### calendar list

List calendar events with date range options.

```bash
h8 calendar list [OPTIONS]

Options:
  --days, -d <DAYS>      Number of days to show (default: 7)
  --from <DATE>          Start date (ISO format)
  --to <DATE>            End date (ISO format)
```

**Examples:**
```bash
h8 calendar list
h8 calendar list --days 14
h8 calendar list --from 2026-01-20 --to 2026-01-25
h8 calendar list --json
```

### calendar add

Add event with natural language input. Supports attendees via "with" keyword.

```bash
h8 calendar add <INPUT>... [OPTIONS]

Arguments:
  INPUT...  Natural language event description

Options:
  --duration, -d <MINUTES>  Default duration in minutes (default: 60)
  --location, -l <LOCATION> Event location
```

**Natural Language Format:**
```
[day/date] [time] [title] [with attendee@email.com]
```

**Examples:**
```bash
# Simple event
h8 calendar add friday 2pm Team Sync

# With quoted title
h8 calendar add 'friday 2pm "Sprint Planning"'

# With attendee (sends meeting invite)
h8 calendar add 'friday 2pm Meeting with alice@example.com'
h8 calendar add 'tomorrow 10am Sync with bob@example.com'

# Multiple words without quotes work too
h8 calendar add friday 2pm Project Status Update

# With duration (default is 60 min)
h8 calendar add 'tomorrow 9am Standup' --duration 15

# With location
h8 calendar add 'friday 2pm Team Sync' --location "Conference Room A"
h8 calendar add 'tomorrow 3pm Client Call' --location "Zoom"

# Time range (explicit end time)
h8 calendar add 'friday 2pm-4pm Workshop'
```

### calendar create

Create event from JSON input (for complex events).

```bash
h8 calendar create

# JSON schema:
{
  "subject": "Meeting Title",        # Required
  "start": "2026-01-22T14:00:00",   # Required: ISO datetime
  "end": "2026-01-22T15:00:00",     # Required: ISO datetime
  "location": "Room A"              # Optional
}
```

**Examples:**
```bash
echo '{"subject": "Team Meeting", "start": "2026-01-22T14:00:00", "end": "2026-01-22T15:00:00", "location": "Room A"}' | h8 calendar create
```

### calendar delete

Delete a calendar event.

```bash
h8 calendar delete --id <EVENT_ID>
```

---

## Free Slots Command

Find free slots in your own calendar.

```bash
h8 free [OPTIONS]

Options:
  --weeks, -w <WEEKS>        Number of weeks to look at (default: 1)
  --duration, -d <MINUTES>   Minimum slot duration in minutes (default: 30)
  --limit, -l <LIMIT>        Maximum number of slots to return
```

**Examples:**
```bash
h8 free
h8 free --weeks 2
h8 free --duration 60 --limit 10
h8 free --json
```

---

## People (ppl) Commands

### ppl free

Find free slots in another person's calendar.

```bash
h8 ppl free <PERSON> [OPTIONS]

Arguments:
  PERSON  Person alias or email address

Options:
  --weeks, -w <WEEKS>        Number of weeks to check (default: 1)
  --duration, -d <MINUTES>   Minimum slot duration in minutes (default: 30)
  --limit, -l <LIMIT>        Limit number of results
```

**Examples:**
```bash
h8 ppl free alice@example.com
h8 ppl free alice@example.com --weeks 2
h8 ppl free alice@example.com --duration 120  # Find 2-hour slots
h8 ppl free alice@example.com --json
```

### ppl agenda

View another person's calendar events.

```bash
h8 ppl agenda <PERSON> [OPTIONS]

Arguments:
  PERSON  Person alias or email address

Options:
  --days, -d <DAYS>   Number of days to show (default: 7)
  --from <DATE>       Start date (ISO format)
  --to <DATE>         End date (ISO format)
```

**Examples:**
```bash
h8 ppl agenda alice@example.com
h8 ppl agenda alice@example.com --days 14
h8 ppl agenda alice@example.com --json
```

### ppl common

Find common free slots between multiple people.

```bash
h8 ppl common <PERSON1> <PERSON2> [PERSON3...] [OPTIONS]

Arguments:
  PEOPLE  Two or more person aliases or email addresses

Options:
  --weeks, -w <WEEKS>        Number of weeks to look at (default: 1)
  --duration, -d <MINUTES>   Minimum slot duration in minutes (default: 30)
  --limit, -l <LIMIT>        Maximum number of slots to return
```

**Examples:**
```bash
h8 ppl common alice@example.com bob@example.com
h8 ppl common alice@example.com bob@example.com charlie@example.com
h8 ppl common alice@example.com bob@example.com --weeks 2 --duration 60
h8 ppl common alice@example.com bob@example.com --json
```

---

## Contacts Commands

### contacts list

List contacts.

```bash
h8 contacts list [OPTIONS]

Options:
  --limit, -l <LIMIT>   Max contacts (default: 100)
  --search, -s <QUERY>  Search by name or email
```

### contacts get

Get a contact by ID.

```bash
h8 contacts get --id <CONTACT_ID>
```

### contacts create

Create a contact from JSON input.

```bash
h8 contacts create
```

### contacts delete

Delete a contact.

```bash
h8 contacts delete --id <CONTACT_ID>
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
h8 calendar show today --json
h8 ppl free alice@example.com --json
```

---

## Tips and Tricks

### Using JSON output for scripting

```bash
# Get list of unread message IDs
h8 mail list --unread --json | jq -r '.[].id'

# Get today's meeting subjects
h8 calendar show today --json | jq -r '.[].subject'
```

### Heredoc for complex JSON

When sending emails with complex content, use heredoc:

```bash
cat <<'EOF' | h8 mail send
{
  "to": ["team@example.com"],
  "subject": "Weekly Update",
  "body": "Hi team,\n\nHere are this week's updates:\n\n1. Item one\n2. Item two\n\nBest regards"
}
EOF
```

### Person aliases

h8 supports person aliases configured in your config.toml. Use aliases instead of full emails:

```bash
h8 ppl free alice           # Uses alias "alice" from config
h8 ppl agenda bob           # Uses alias "bob" from config
```
