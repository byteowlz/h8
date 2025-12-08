# h8

Rust CLI for MS365 Exchange Web Services (EWS) covering calendar, mail, contacts, and free-slot search. Works when Graph or IMAP are blocked but EWS is available.

## Requirements

- Rust stable toolchain
- Python 3.12+ with `uv`
- [oama](https://github.com/pstrahl/oama) to issue OAuth2 access tokens (`oama access <email>`)

## Architecture

- A Python service (FastAPI) talks to EWS via `exchangelib`, refreshes data periodically, and caches it locally.
- The Rust CLI calls the local service for all calendar/mail/contact/free-slot operations.

## Setup

```bash
# Install deps
just install

# Start the Python service (in another shell)
just service-start  # defaults to 127.0.0.1:8787 (logs to state dir)

# Stop / status
just service-stop
just service-status

# Build/run CLI
cargo build --manifest-path h8/Cargo.toml
```

## Configuration

Default path: `$XDG_CONFIG_HOME/h8/config.toml` (or `~/.config/h8/config.toml`). Overrides: local `./config.toml`, env (`H8__...`), then `--config`. CLI flags take precedence.

```toml
account = "your.email@example.com"
timezone = "Europe/Berlin"
service_url = "http://127.0.0.1:8787"

[free_slots]
start_hour = 9
end_hour = 17
exclude_weekends = true
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
```

Add `--json` for machine-readable output. Use `--account` to target another mailbox. Ensure the Python service is running first.

## License

MIT
