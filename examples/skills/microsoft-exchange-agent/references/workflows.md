# Microsoft Exchange Workflows

Common workflows for email and calendar management using h8.

## Email Workflows

### 1. Daily Email Check

**Scenario**: Start of day email review

```bash
# List recent emails
h8 mail list

# Check for specific sender
h8 mail search "boss@example.com"

# Read important emails
h8 mail read <message-id>

# Reply to urgent items
h8 mail reply <message-id>
```

### 2. Email Triage

**Scenario**: Process inbox and categorize emails

```bash
# List all emails
h8 mail list

# Search by topic
h8 mail search "project alpha"

# Mark as read
h8 mail mark <message-id> read

# Delete spam/unnecessary
h8 mail delete <message-id>

# Forward to team
h8 mail forward <message-id>
```

### 3. Scheduled Email Campaign

**Scenario**: Send scheduled emails to team

```bash
# Schedule morning standup reminder
h8 mail send --to team@example.com \
  --subject "Daily Standup - 9am" \
  --body "Don't forget our daily standup at 9am!" \
  --schedule "tomorrow 8:30am"

# Schedule weekly report
h8 mail send --to manager@example.com \
  --subject "Weekly Report" \
  --body "This week's progress..." \
  --schedule "friday 4pm"
```

### 4. Bulk Email with Attachments

**Scenario**: Send email with multiple recipients and attachments

```bash
# Send to multiple recipients
h8 mail send \
  --to person1@example.com \
  --to person2@example.com \
  --cc manager@example.com \
  --bcc archive@example.com \
  --subject "Project Documentation" \
  --body "Please find attached..."
```

---

## Calendar Workflows

### 1. Daily Planning

**Scenario**: Review and plan your day

```bash
# Check today's agenda
h8 agenda

# Check tomorrow
h8 calendar show "tomorrow"

# Check next week
h8 calendar show "next week"

# Add new task
h8 calendar add "today 3pm Review documentation" --duration 30
```

### 2. Meeting Scheduling

**Scenario**: Schedule a meeting with multiple attendees

```bash
# Step 1: Check your availability
h8 agenda

# Step 2: Check attendees' availability
h8 ppl free alice@example.com
h8 ppl free bob@example.com

# Step 3: Find common time
h8 ppl common alice@example.com bob@example.com

# Step 4: Send meeting invite (creates event and sends invitations automatically)
h8 calendar invite --subject "Team Planning Meeting" \
  --start 2026-01-24T14:00:00 --end 2026-01-24T15:00:00 \
  --to alice@example.com --to bob@example.com \
  --location "Conference Room A" \
  --body "Weekly team planning session"
```

### 2b. Responding to Meeting Invites

**Scenario**: Review and respond to incoming meeting invites

```bash
# Step 1: List pending meeting invites
h8 calendar invites

# Step 2: Review your schedule for conflicts
h8 agenda

# Step 3: Accept invite
h8 calendar rsvp <invite-id> accept

# Or decline with explanation
h8 calendar rsvp <invite-id> decline --message "I have a conflict at this time"

# Or tentatively accept
h8 calendar rsvp <invite-id> tentative --message "Checking if I can reschedule another meeting"
```

### 3. Recurring Event Setup

**Scenario**: Set up weekly team meetings

```bash
# Monday standup
h8 calendar add "next monday 9am Daily Standup" \
  --location "Zoom" \
  --duration 15

# Friday retrospective
h8 calendar add "next friday 4pm Weekly Retrospective" \
  --location "Conference Room B" \
  --duration 60
```

### 4. Event Conflict Resolution

**Scenario**: Check for conflicts and reschedule

```bash
# Check agenda
h8 agenda

# Search for specific meeting
h8 calendar search "client meeting"

# Delete old event
h8 calendar delete <event-id>

# Create new meeting invite at different time
h8 calendar invite --subject "Client Meeting - Rescheduled" \
  --start 2026-01-22T15:00:00 --end 2026-01-22T16:00:00 \
  --to client@example.com \
  --location "Zoom" \
  --body "Meeting rescheduled to a new time"
```

---

## Combined Email + Calendar Workflows

### 1. Meeting Follow-up

**Scenario**: After a meeting, send summary and schedule follow-up

```bash
# Send meeting summary
h8 mail send \
  --to attendees@example.com \
  --subject "Meeting Summary - Project Alpha" \
  --body "Today's meeting discussed... Action items:..."

# Schedule follow-up meeting
h8 calendar add "next week monday 2pm Project Alpha Follow-up" \
  --location "Zoom"
```

### 2. Out of Office Setup

**Scenario**: Set out of office and notify team

```bash
# Check upcoming commitments
h8 calendar show "next week"

# Send OOO notification
h8 mail send \
  --to team@example.com \
  --subject "Out of Office - Next Week" \
  --body "I'll be out of office next week. Contact bob@example.com for urgent matters."

# Block calendar (create all-day events)
h8 calendar add "next monday all-day Out of Office"
h8 calendar add "next tuesday all-day Out of Office"
h8 calendar add "next wednesday all-day Out of Office"
```

### 3. Project Coordination

**Scenario**: Coordinate project timeline with team

```bash
# Check team availability
h8 ppl common alice@example.com bob@example.com charlie@example.com

# Schedule kickoff meeting with invites
h8 calendar invite --subject "Project Kickoff" \
  --start 2026-01-24T10:00:00 --end 2026-01-24T12:00:00 \
  --to alice@example.com --to bob@example.com --to charlie@example.com \
  --optional manager@example.com \
  --location "Conference Room A" \
  --body "Project kickoff meeting - please review the timeline document before attending"

# Schedule milestone reviews
h8 calendar invite --subject "Milestone 1 Review" \
  --start 2026-02-07T14:00:00 --end 2026-02-07T15:00:00 \
  --to alice@example.com --to bob@example.com --to charlie@example.com

h8 calendar invite --subject "Milestone 2 Review" \
  --start 2026-02-21T14:00:00 --end 2026-02-21T15:00:00 \
  --to alice@example.com --to bob@example.com --to charlie@example.com
```

### 4. Weekly Planning Routine

**Scenario**: Sunday evening planning for the week

```bash
# Review next week's calendar
h8 calendar show "next week"

# Check for conflicts
h8 agenda

# Review recent emails
h8 mail list

# Search for action items
h8 mail search "action item"

# Schedule focus time
h8 calendar add "next week monday 9am-11am Deep Work - No Meetings" --location "Office"
h8 calendar add "next week wednesday 2pm-4pm Deep Work - No Meetings" --location "Office"

# Send weekly plan to manager
h8 mail send \
  --to manager@example.com \
  --subject "Next Week's Plan" \
  --body "My focus areas for next week..."
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

# Schedule at identified time with meeting invite
h8 calendar invite --subject "Team Sync" \
  --start 2026-01-23T15:00:00 --end 2026-01-23T16:00:00 \
  --to alice@example.com --to bob@example.com --to charlie@example.com \
  --location "Conference Room B"
```

### 2. One-on-One Scheduling

**Scenario**: Schedule regular one-on-ones

```bash
# Check manager's availability
h8 ppl free manager@example.com

# Schedule one-on-one with meeting invite
h8 calendar invite --subject "One-on-One" \
  --start 2026-01-27T10:00:00 --end 2026-01-27T10:30:00 \
  --to manager@example.com \
  --location "Manager's Office" \
  --body "Weekly one-on-one meeting"
```

### 3. External Meeting Coordination

**Scenario**: Schedule meeting with external client

```bash
# Check your availability
h8 agenda

# Propose times to client
h8 mail send \
  --to client@external.com \
  --subject "Meeting Request" \
  --body "I'm available: Friday 2pm, Monday 10am, or Tuesday 3pm. What works best for you?"

# After confirmation, send meeting invite
h8 calendar invite --subject "Client Meeting - Project Review" \
  --start 2026-01-24T14:00:00 --end 2026-01-24T15:00:00 \
  --to client@external.com \
  --location "Zoom - https://zoom.us/j/123456789" \
  --body "Looking forward to discussing the project"
```

---

## Advanced Workflows

### 1. Email Search and Analysis

**Scenario**: Track project communications

```bash
# Search for project-related emails
h8 mail search "project alpha" --json > project_emails.json

# Search for specific person
h8 mail search "alice@example.com" --json

# Search by date range (if supported)
h8 mail search "last week"
```

### 2. Calendar Export for Reporting

**Scenario**: Generate weekly report of meetings

```bash
# Get week's calendar in JSON
h8 calendar show "this week" --json > week_calendar.json

# Get agenda in YAML
h8 agenda --yaml > today_agenda.yaml
```

### 3. Automated Reminders

**Scenario**: Set up automated reminders for deadlines

```bash
# Deadline reminder - 1 week before
h8 mail send \
  --to team@example.com \
  --subject "Reminder: Project Deadline in 1 Week" \
  --body "Project Alpha deadline is next Friday" \
  --schedule "next friday 9am"

# Day-before reminder
h8 mail send \
  --to team@example.com \
  --subject "Reminder: Project Deadline Tomorrow" \
  --body "Final reminder: Project Alpha is due tomorrow" \
  --schedule "thursday 9am"
```

---

## Best Practices

### Email Best Practices

1. **Check before sending**: Review recipient list, subject, and body
2. **Use search**: Find related emails before responding
3. **Schedule wisely**: Use scheduled send for optimal delivery times
4. **Keep it professional**: Maintain appropriate tone and formatting

### Calendar Best Practices

1. **Buffer time**: Add 5-10 minute buffers between meetings
2. **Location clarity**: Always specify location (room or Zoom link)
3. **Duration accuracy**: Set realistic meeting durations
4. **Check conflicts**: Review agenda before adding events

### Availability Best Practices

1. **Check early**: Check availability well in advance
2. **Multiple options**: Provide multiple time options
3. **Time zones**: Account for different time zones
4. **Confirmation**: Always confirm scheduled meetings

### Workflow Optimization

1. **Morning routine**: Check email and agenda first thing
2. **Batch processing**: Process emails in batches, not continuously
3. **Focus time**: Block calendar for deep work
4. **End-of-day review**: Review tomorrow's schedule before leaving
