# Microsoft Exchange Workflows

Common workflows for email and calendar management using h8.

## Email Workflows

### 1. Daily Email Check

**Scenario**: Start of day email review

```bash
# List recent emails
h8 mail list

# List only unread emails
h8 mail list --unread

# Get full content of specific email
h8 mail get --id <message-id>
```

### 2. Email Triage

**Scenario**: Process inbox and categorize emails

```bash
# List all emails
h8 mail list

# List with more results
h8 mail list --limit 50

# Get specific email details
h8 mail get --id <message-id>

# Get emails in JSON for processing
h8 mail list --json
```

### 3. Send Email

**Scenario**: Send emails to team

```bash
# Send simple email
echo '{"to": ["team@example.com"], "subject": "Daily Standup - 9am", "body": "Don'\''t forget our daily standup at 9am!"}' | h8 mail send

# Scheduled email (deliver tomorrow morning)
echo '{"to": ["team@example.com"], "subject": "Daily Standup Reminder", "body": "Standup in 30 minutes!", "schedule_at": "2026-01-22T08:30:00"}' | h8 mail send

# Using heredoc for complex body
cat <<'EOF' | h8 mail send
{
  "to": ["manager@example.com"],
  "subject": "Weekly Report",
  "body": "Hi,\n\nHere is this week's progress:\n\n1. Completed feature X\n2. Fixed bug Y\n3. Started work on Z\n\nBest regards"
}
EOF
```

### 4. Email with Multiple Recipients

**Scenario**: Send email to multiple people

```bash
# Multiple recipients with CC
cat <<'EOF' | h8 mail send
{
  "to": ["person1@example.com", "person2@example.com"],
  "cc": ["manager@example.com"],
  "subject": "Project Documentation",
  "body": "Please find the documentation details below..."
}
EOF

# HTML email
cat <<'EOF' | h8 mail send
{
  "to": ["team@example.com"],
  "subject": "Weekly Newsletter",
  "body": "<h1>Weekly Update</h1><p>Here are this week's highlights...</p><ul><li>Item 1</li><li>Item 2</li></ul>",
  "html": true
}
EOF
```

---

## Calendar Workflows

### 1. Daily Planning

**Scenario**: Review and plan your day

```bash
# Check today's events
h8 calendar show today

# Check tomorrow
h8 calendar show tomorrow

# Check specific day
h8 calendar show friday

# Check next week
h8 calendar show "next week"

# Add new task
h8 calendar add 'today 3pm Review documentation' --duration 30
```

### 2. Meeting Scheduling with Attendees

**Scenario**: Schedule a meeting with multiple attendees

```bash
# Step 1: Check your schedule
h8 calendar show today
h8 calendar show tomorrow

# Step 2: Check attendees' availability
h8 ppl free alice@example.com
h8 ppl free bob@example.com

# Step 3: Find common time for everyone
h8 ppl common alice@example.com bob@example.com

# Step 4: Create meeting with attendees (sends invites automatically)
h8 calendar add 'friday 2pm "Team Planning Meeting" with alice@example.com' --location "Conference Room A"

# For multiple attendees
h8 calendar add 'monday 10am Sprint Planning with alice@example.com' --location "Zoom"
```

### 3. Recurring Event Setup

**Scenario**: Set up weekly team meetings

```bash
# Monday standup
h8 calendar add 'next monday 9am Daily Standup' --location "Zoom" --duration 15

# Friday retrospective
h8 calendar add 'next friday 4pm Weekly Retrospective' --location "Conference Room B" --duration 60
```

### 4. Event Management

**Scenario**: Check and manage existing events

```bash
# Check schedule
h8 calendar show today

# List events for date range
h8 calendar list --days 14

# Get event details in JSON
h8 calendar show today --json

# Delete event (get ID from list output)
h8 calendar delete --id <event-id>

# Create new event at different time
h8 calendar add 'friday 3pm Client Meeting - Rescheduled' --location "Zoom"
```

---

## Combined Email + Calendar Workflows

### 1. Meeting Follow-up

**Scenario**: After a meeting, send summary and schedule follow-up

```bash
# Send meeting summary
cat <<'EOF' | h8 mail send
{
  "to": ["alice@example.com", "bob@example.com"],
  "subject": "Meeting Summary - Project Alpha",
  "body": "Hi team,\n\nToday's meeting discussed:\n- Feature priorities\n- Timeline adjustments\n\nAction items:\n1. Alice: Review spec by Friday\n2. Bob: Update documentation\n\nBest regards"
}
EOF

# Schedule follow-up meeting
h8 calendar add 'next monday 2pm "Project Alpha Follow-up" with alice@example.com' --location "Zoom"
```

### 2. Out of Office Setup

**Scenario**: Set out of office and notify team

```bash
# Check upcoming commitments
h8 calendar show "next week"

# Send OOO notification
cat <<'EOF' | h8 mail send
{
  "to": ["team@example.com"],
  "subject": "Out of Office - Next Week",
  "body": "Hi team,\n\nI'll be out of office next week (Jan 27-31).\n\nFor urgent matters, please contact bob@example.com.\n\nBest regards"
}
EOF

# Block calendar (create events)
h8 calendar add 'next monday Out of Office' --duration 480
h8 calendar add 'next tuesday Out of Office' --duration 480
h8 calendar add 'next wednesday Out of Office' --duration 480
```

### 3. Project Coordination

**Scenario**: Coordinate project timeline with team

```bash
# Check team availability
h8 ppl common alice@example.com bob@example.com charlie@example.com

# Schedule kickoff meeting
h8 calendar add 'friday 10am "Project Kickoff" with alice@example.com' --location "Conference Room A" --duration 120

# Send project brief
cat <<'EOF' | h8 mail send
{
  "to": ["alice@example.com", "bob@example.com", "charlie@example.com"],
  "subject": "Project Kickoff - Friday 10am",
  "body": "Hi team,\n\nI've scheduled our project kickoff for Friday 10am.\n\nPlease review the timeline document before attending.\n\nAgenda:\n1. Project overview\n2. Role assignments\n3. Timeline review\n4. Q&A\n\nBest regards"
}
EOF
```

### 4. Weekly Planning Routine

**Scenario**: Sunday evening planning for the week

```bash
# Review next week's calendar
h8 calendar show "next week"

# Check today's remaining items
h8 calendar show today

# Review recent emails
h8 mail list --limit 30

# Check for unread emails
h8 mail list --unread

# Schedule focus time
h8 calendar add 'next monday 9am-11am Deep Work - No Meetings' --location "Office"
h8 calendar add 'next wednesday 2pm-4pm Deep Work - No Meetings' --location "Office"

# Send weekly plan to manager
cat <<'EOF' | h8 mail send
{
  "to": ["manager@example.com"],
  "subject": "Next Week's Plan",
  "body": "Hi,\n\nMy focus areas for next week:\n\n1. Complete feature X documentation\n2. Review pull requests\n3. Meet with team on project timeline\n\nBest regards"
}
EOF
```

---

## Availability Management Workflows

### 1. Check Team Availability

**Scenario**: Find when entire team is available

```bash
# Check individual availability
h8 ppl free alice@example.com --weeks 2
h8 ppl free bob@example.com --weeks 2
h8 ppl free charlie@example.com --weeks 2

# Find common slots
h8 ppl common alice@example.com bob@example.com charlie@example.com

# Schedule at identified time
h8 calendar add 'friday 3pm "Team Sync" with alice@example.com' --location "Conference Room B"
```

### 2. One-on-One Scheduling

**Scenario**: Schedule regular one-on-ones

```bash
# Check manager's availability
h8 ppl free manager@example.com

# View manager's agenda (if you have access)
h8 ppl agenda manager@example.com

# Schedule one-on-one
h8 calendar add 'monday 10am "One-on-One" with manager@example.com' --location "Manager'\''s Office" --duration 30
```

### 3. External Meeting Coordination

**Scenario**: Schedule meeting with external client

```bash
# Check your availability
h8 calendar show today
h8 calendar show "next week"

# Find your free slots
h8 free --weeks 2

# Propose times to client
cat <<'EOF' | h8 mail send
{
  "to": ["client@external.com"],
  "subject": "Meeting Request - Project Review",
  "body": "Hi,\n\nI'd like to schedule a meeting to discuss the project.\n\nI'm available:\n- Friday 2pm\n- Monday 10am\n- Tuesday 3pm\n\nWhat works best for you?\n\nBest regards"
}
EOF

# After confirmation, create meeting
h8 calendar add 'friday 2pm "Client Meeting - Project Review" with client@external.com' --location "Zoom - https://zoom.us/j/123456789"
```

---

## Advanced Workflows

### 1. Calendar Export for Reporting

**Scenario**: Generate weekly report of meetings

```bash
# Get week's calendar in JSON
h8 calendar show "this week" --json > week_calendar.json

# Get specific day in JSON
h8 calendar show today --json > today_agenda.json

# Process with jq (if available)
h8 calendar show today --json | jq -r '.[].subject'
```

### 2. Automated Scheduled Emails

**Scenario**: Set up reminders for deadlines

```bash
# Deadline reminder - 1 week before
cat <<'EOF' | h8 mail send
{
  "to": ["team@example.com"],
  "subject": "Reminder: Project Deadline in 1 Week",
  "body": "Hi team,\n\nThis is a reminder that the Project Alpha deadline is next Friday.\n\nPlease ensure all tasks are on track.\n\nBest regards",
  "schedule_at": "2026-01-24T09:00:00"
}
EOF

# Day-before reminder
cat <<'EOF' | h8 mail send
{
  "to": ["team@example.com"],
  "subject": "Reminder: Project Deadline Tomorrow",
  "body": "Hi team,\n\nFinal reminder: Project Alpha is due tomorrow.\n\nPlease complete any outstanding items today.\n\nBest regards",
  "schedule_at": "2026-01-30T09:00:00"
}
EOF
```

### 3. Email Attachments

**Scenario**: Work with email attachments

```bash
# List attachments for an email
h8 mail attachments --id <message-id>

# Download specific attachment
h8 mail attachments --id <message-id> --download 0 --output ./downloads/

# Get attachment list in JSON
h8 mail attachments --id <message-id> --json
```

---

## Best Practices

### Email Best Practices

1. **Check before sending**: Review recipient list, subject, and body
2. **Use JSON correctly**: Ensure valid JSON format when piping to h8 mail send
3. **Schedule wisely**: Use schedule_at for optimal delivery times
4. **Keep it professional**: Maintain appropriate tone and formatting
5. **Use heredoc**: For multi-line emails, use heredoc to avoid escaping issues

### Calendar Best Practices

1. **Buffer time**: Add 5-10 minute buffers between meetings
2. **Location clarity**: Always specify location (room or Zoom link)
3. **Duration accuracy**: Set realistic meeting durations
4. **Check conflicts**: Review calendar before adding events
5. **Use natural language**: Let h8 calendar add parse date/time naturally

### Availability Best Practices

1. **Check early**: Check availability well in advance
2. **Multiple options**: Provide multiple time options
3. **Find common slots**: Use `h8 ppl common` for group scheduling
4. **Confirm meetings**: Always confirm scheduled meetings via email

### Workflow Optimization

1. **Morning routine**: Check email and calendar first thing
2. **Batch processing**: Process emails in batches, not continuously
3. **Focus time**: Block calendar for deep work
4. **End-of-day review**: Review tomorrow's schedule before leaving
5. **JSON for scripting**: Use --json output for automated processing
