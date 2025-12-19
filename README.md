![banner](banner.png)

# h8

Rust CLI for MS365 Exchange Web Services (EWS) covering calendar, mail, contacts, and free-slot search. Works when Graph or IMAP are blocked but EWS is available.

## Requirements

- Rust stable toolchain
- Python 3.12+ with `uv`
- [oama](https://github.com/pdobsan/oama) - OAuth credential manager for issuing access tokens

### oama Setup

h8 relies on oama to obtain OAuth2 access tokens for Microsoft 365. You must configure oama before using h8:

1. Install oama (see [oama README](https://github.com/pdobsan/oama#installation))
2. Configure your Microsoft 365 account in oama's config file
3. Complete the initial OAuth2 authorization flow: `oama authorize <email>`
4. Verify tokens work: `oama access <email>` should print an access token

h8's Python service calls `oama access <email>` to get fresh tokens as needed.

## Architecture

- A Python service (FastAPI) talks to EWS via `exchangelib`, refreshes data periodically, and caches it locally.
- The Rust CLI calls the local service for all calendar/mail/contact/free-slot operations.

## Setup

```bash
# Install Rust CLI and Python deps
just install

# Install the Python service globally (enables `h8 service start` from anywhere)
cd ~/path/to/h8
uv tool install -e .

# Start the Python service
h8 service start   # runs in background, logs to ~/.local/state/h8/service.log

# Check status / stop
h8 service status
h8 service stop
h8 service restart
```

## Configuration

Default path: `$XDG_CONFIG_HOME/h8/config.toml` (or `~/.config/h8/config.toml`). Overrides: local `./config.toml`, env (`H8__...`), then `--config`. CLI flags take precedence.

```toml
account = "your.email@example.com"
timezone = "Europe/Berlin"
service_url = "http://127.0.0.1:8787"

[calendar]
default_view = "list"  # list, gantt, or compact

[free_slots]
start_hour = 9
end_hour = 17
exclude_weekends = true

[people]
Roman = "roman.kowalski@example.com"
Alice = "alice.smith@example.com"
```

See `examples/config.toml` for a fuller template.

## Logging

- CLI: `RUST_LOG=debug` (or `--debug`/`--trace`) for verbose output.
- Service: `H8_SERVICE_LOGLEVEL` (`INFO`/`DEBUG`), `H8_SERVICE_CACHE_TTL` (seconds), and `H8_SERVICE_REFRESH_SECONDS` control caching/refresh.

## Usage

```bash
# Calendar
h8 calendar list --days 7
h8 cal ls --days 3  # alias for calendar list
echo '{"subject":"1:1","start":"2025-01-15T10:00:00+01:00","end":"2025-01-15T10:30:00+01:00"}' | h8 calendar create
h8 calendar delete --id "<event-id>"
h8 agenda  # today's timeline view in your configured timezone

# Mail
h8 mail list --folder inbox --limit 10 --unread
h8 mail get --id "<message-id>"
echo '{"to":["user@example.com"],"subject":"Hello","body":"Hi there"}' | h8 mail send
h8 mail fetch --folder inbox --output ~/Mail/work --format maildir

# Contacts
h8 contacts list --search john --limit 25
h8 contacts get --id "<contact-id>"
echo '{"display_name":"John Doe","email":"john@example.com"}' | h8 contacts create

# Free slots
h8 free --weeks 2 --duration 60

# Other people's calendars (requires [people] aliases in config or full email)
h8 ppl agenda Roman --days 7          # View Roman's calendar
h8 ppl free Roman --weeks 2           # Find Roman's free slots
h8 ppl common Roman Alice --weeks 2   # Find common free slots between Roman and Alice
```

Add `--json` for machine-readable output. Use `--account` to target another mailbox. Ensure the Python service is running first.

## License

MIT
