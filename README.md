# h8

CLI for MS365 Exchange Web Services (EWS) - calendar, mail, and contacts.

Useful when IMAP and Graph API are blocked but EWS is allowed.

## Requirements

- Python 3.12+
- [oama](https://github.com/pstrahl/oama) for OAuth2 token management

## Installation

```bash
uv pip install -e .
```

## Configuration

Config file: `~/.config/h8/config.toml` (or `$XDG_CONFIG_HOME/h8/config.toml`)

```toml
account = "your.email@example.com"
timezone = "Europe/Berlin"

[free_slots]
start_hour = 9
end_hour = 17
exclude_weekends = true
```

See `examples/config.toml` for a complete example.

## Usage

### Calendar

```bash
# List events for the next 7 days
h8 calendar list

# List events for specific date range
h8 cal ls --from 2025-01-01 --to 2025-01-31

# Create event (JSON from stdin)
echo '{"subject": "Meeting", "start": "2025-01-15T10:00:00", "end": "2025-01-15T11:00:00"}' | h8 cal create

# Delete event
h8 cal delete --id <event_id>
```

### Mail

```bash
# List recent messages
h8 mail list

# List unread messages only
h8 mail ls --unread --limit 10

# Get full message
h8 mail get --id <message_id>

# Fetch to maildir
h8 mail fetch --output ~/Mail/work --format maildir

# Send email (JSON from stdin)
echo '{"to": "user@example.com", "subject": "Hello", "body": "..."}' | h8 mail send
```

### Contacts

```bash
# List contacts
h8 contacts list

# Search contacts
h8 contacts ls --search "john"

# Get contact details
h8 contacts get --id <contact_id>

# Create contact (JSON from stdin)
echo '{"display_name": "John Doe", "email": "john@example.com"}' | h8 contacts create

# Delete contact
h8 contacts delete --id <contact_id>
```

### Free Slots

```bash
# Find 30-minute free slots in the current week
h8 free

# Find 60-minute slots in the next 2 weeks
h8 free --weeks 2 --duration 60
```

### Common Options

- `--json` / `-j`: Output as JSON
- `--account` / `-a`: Override email account

## License

MIT
