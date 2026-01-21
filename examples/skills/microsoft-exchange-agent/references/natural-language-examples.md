# Natural Language Examples for h8 Calendar

h8 calendar add supports flexible natural language parsing. This document provides comprehensive examples.

## Time Formats

### Relative Time

```bash
# Today
h8 calendar add "today 2pm Team Meeting"
h8 calendar add "today 14:00 Project Review"
h8 calendar add "today noon Lunch with Client"

# Tomorrow
h8 calendar add "tomorrow 9am Standup"
h8 calendar add "tomorrow 3pm Client Call"
h8 calendar add "tomorrow afternoon Team Sync"

# In X hours/days
h8 calendar add "in 2 hours Quick Sync"
h8 calendar add "in 3 days Project Review"
```

### Named Days

```bash
# Specific day of week
h8 calendar add "monday 10am Sprint Planning"
h8 calendar add "friday 4pm Retrospective"
h8 calendar add "wednesday noon Team Lunch"

# Next/This week
h8 calendar add "next monday 9am Weekly Standup"
h8 calendar add "next friday 2pm Demo Day"
h8 calendar add "this thursday 3pm Review Meeting"
```

### Absolute Dates

```bash
# Month and day
h8 calendar add "jan 25 2pm Budget Review"
h8 calendar add "february 14 6pm Valentine's Dinner"

# Full date
h8 calendar add "2026-01-25 10am Quarterly Review"
h8 calendar add "25.01.2026 14:00 Client Meeting"
```

## Time of Day Formats

### 12-Hour Format

```bash
h8 calendar add "friday 9am Morning Standup"
h8 calendar add "friday 2pm Afternoon Review"
h8 calendar add "friday 11am Late Morning Sync"
```

### 24-Hour Format

```bash
h8 calendar add "friday 09:00 Morning Standup"
h8 calendar add "friday 14:00 Afternoon Review"
h8 calendar add "friday 16:30 Late Afternoon Sync"
```

### Special Times

```bash
h8 calendar add "friday noon Lunch Meeting"
h8 calendar add "friday midnight Deployment"
h8 calendar add "friday morning Team Sync"  # Uses default morning time
h8 calendar add "friday afternoon Review"   # Uses default afternoon time
```

## Duration Specifications

### Explicit Duration in Command

```bash
# Time range
h8 calendar add "friday 2pm-4pm Workshop"
h8 calendar add "monday 9am-11:30am Training Session"
h8 calendar add "tuesday 14:00-15:30 Client Meeting"

# Duration with --duration flag
h8 calendar add "friday 2pm Team Meeting" --duration 30
h8 calendar add "monday 9am Training" --duration 120
h8 calendar add "wednesday 3pm Review" --duration 45
```

### Implicit Duration

```bash
# Uses default duration (60 minutes)
h8 calendar add "friday 2pm Team Meeting"
```

## Location Examples

```bash
# Physical locations
h8 calendar add "friday 2pm Team Meeting" --location "Conference Room A"
h8 calendar add "monday 10am Standup" --location "Office - 3rd Floor"
h8 calendar add "thursday 3pm Review" --location "Building 2, Room 305"

# Virtual locations
h8 calendar add "friday 2pm Remote Sync" --location "Zoom"
h8 calendar add "monday 9am Standup" --location "Microsoft Teams"
h8 calendar add "wednesday 4pm Client Call" --location "Google Meet"

# Links
h8 calendar add "friday 2pm Demo" --location "https://zoom.us/j/123456789"
```

## Event Titles

### Simple Titles

```bash
h8 calendar add "friday 2pm Standup"
h8 calendar add "monday 10am Review"
h8 calendar add "thursday 3pm Demo"
```

### Quoted Titles (for complex names)

```bash
h8 calendar add 'friday 2pm "Project Alpha - Sprint Review"'
h8 calendar add 'monday 10am "Q1 Planning: Team A & Team B"'
h8 calendar add 'wednesday 3pm "Client Call: Feature Discussion"'
```

### Titles with Attendees

```bash
h8 calendar add "friday 2pm Meeting with alice@example.com"
h8 calendar add "monday 10am Sync with bob and charlie"
h8 calendar add "thursday 3pm Review with team@example.com"
```

## Complex Examples

### Full Featured Events

```bash
# Event with all details
h8 calendar add "next friday 2pm-4pm Team Workshop with alice@example.com" \
  --location "Conference Room A"

# Multi-day event (if supported)
h8 calendar add "monday 9am Company Offsite" --duration 480  # 8 hours

# Recurring pattern (create multiple)
h8 calendar add "next monday 9am Weekly Standup" --duration 15
h8 calendar add "next tuesday 9am Weekly Standup" --duration 15
h8 calendar add "next wednesday 9am Weekly Standup" --duration 15
```

### Project-Based Events

```bash
# Sprint ceremonies
h8 calendar add "monday 10am Sprint Planning" --duration 120 --location "Zoom"
h8 calendar add "monday 2pm Sprint Review" --duration 60 --location "Conference Room B"
h8 calendar add "friday 4pm Sprint Retrospective" --duration 90 --location "Conference Room A"

# Project milestones
h8 calendar add "february 15 2pm Milestone 1 Review" --duration 120
h8 calendar add "march 15 2pm Milestone 2 Review" --duration 120
```

### Client Meetings

```bash
# Initial meeting
h8 calendar add "next tuesday 10am Client Kickoff Meeting" \
  --duration 60 \
  --location "Zoom - https://zoom.us/j/123456789"

# Follow-up
h8 calendar add "next friday 2pm Client Follow-up" \
  --duration 30 \
  --location "Phone"

# Review
h8 calendar add "next month 10am Client Quarterly Review" \
  --duration 90 \
  --location "Client Office"
```

## Natural Language Patterns

### Day References

| Pattern | Example |
|---------|---------|
| `today` | today 2pm Meeting |
| `tomorrow` | tomorrow 3pm Review |
| `monday`, `tuesday`, etc. | friday 10am Standup |
| `next monday` | next monday 9am Planning |
| `this friday` | this friday 2pm Demo |
| `in X days` | in 3 days 2pm Review |

### Time References

| Pattern | Example |
|---------|---------|
| `9am`, `2pm` | 9am Standup, 2pm Review |
| `09:00`, `14:00` | 09:00 Meeting, 14:00 Call |
| `noon` | noon Lunch Meeting |
| `midnight` | midnight Deployment |
| `morning` | morning Team Sync |
| `afternoon` | afternoon Review |

### Duration Patterns

| Pattern | Example |
|---------|---------|
| `2pm-4pm` | 2pm-4pm Workshop |
| `--duration 30` | --duration 30 (30 minutes) |
| `--duration 120` | --duration 120 (2 hours) |
| Default (no duration) | Uses 60 minutes |

## Calendar Week Examples

```bash
# Using calendar week notation (if supported)
h8 calendar show "kw1"   # Calendar week 1
h8 calendar show "kw30"  # Calendar week 30
h8 calendar show "cw15"  # Calendar week 15
```

## Date Range Examples

```bash
# Show events in range
h8 calendar show "this week"
h8 calendar show "next week"
h8 calendar show "this month"
h8 calendar show "january"
h8 calendar show "next month"
```

## Tips for Natural Language

### Best Practices

1. **Be explicit with time**: Use "2pm" instead of "afternoon"
2. **Use 24-hour for clarity**: "14:00" is less ambiguous than "2pm"
3. **Quote complex titles**: Use quotes for titles with special characters
4. **Specify location**: Always add --location for clarity
5. **Set realistic durations**: Don't use default 60min for short meetings

### Common Mistakes

```bash
# ❌ Ambiguous
h8 calendar add "meeting tomorrow"  # Missing time

# ✅ Clear
h8 calendar add "tomorrow 2pm Team Meeting"

# ❌ Unclear duration
h8 calendar add "friday afternoon workshop"  # Ambiguous time and duration

# ✅ Explicit
h8 calendar add "friday 2pm-5pm Workshop" --location "Conference Room A"

# ❌ Missing location
h8 calendar add "friday 2pm Client Meeting"  # Where?

# ✅ Complete
h8 calendar add "friday 2pm Client Meeting" --location "Zoom"
```

## Advanced Natural Language

### Relative Week References

```bash
# Current week
h8 calendar add "this monday 9am Standup"
h8 calendar add "this friday 4pm Review"

# Next week
h8 calendar add "next monday 9am Planning"
h8 calendar add "next friday 2pm Demo"

# Week after next
h8 calendar add "two weeks monday 10am Quarterly Review"
```

### Month References

```bash
# Current month
h8 calendar add "15th 2pm Mid-Month Review"

# Named months
h8 calendar add "january 25 10am Budget Review"
h8 calendar add "feb 14 6pm Valentine's Dinner"

# Next month
h8 calendar add "next month 1st 9am Monthly Kickoff"
```

## Testing Natural Language

To test if a natural language pattern works:

```bash
# Use --dry-run to test without creating
h8 calendar add "friday 2pm Test Event" --dry-run

# Check created event details
h8 agenda

# Or show specific day
h8 calendar show "friday"
```
