# h8 (Rust)

Rust CLI for Microsoft 365 Exchange Web Services. Supports calendar, mail, contacts, and free-slot discovery using OAuth2 tokens from `oama`.

## Quick Start

```bash
rustup default stable

# Start Python service (in repo root)
just service-start  # uses uv run h8-service; logs to state dir

# Build and run CLI
cargo build --manifest-path h8/Cargo.toml
cargo run --manifest-path h8/Cargo.toml -- --help
```

Install the CLI with `cargo install --path . --locked` if desired; keep the service running alongside it.

## Configuration

- Default: `$XDG_CONFIG_HOME/h8/config.toml` (or `~/.config/h8/config.toml`).
- Local overrides: `./config.toml`.
- Env overrides: `H8__account`, `H8__timezone`, `H8__service_url`, etc.
- CLI overrides: `--config` path and `--account` flag.

Example (`../examples/config.toml`):

```toml
account = "your.email@example.com"
timezone = "Europe/Berlin"
service_url = "http://127.0.0.1:8787"

[free_slots]
start_hour = 9
end_hour = 17
exclude_weekends = true
```

Run `cargo run -- init` to create the default file if it is missing.

## Logging

- CLI: use `RUST_LOG=debug` or `--debug`/`--trace` for more verbosity.
- Service: `H8_SERVICE_LOGLEVEL` (`INFO`/`DEBUG`), `H8_SERVICE_CACHE_TTL`, and `H8_SERVICE_REFRESH_SECONDS` control log level and cache refresh cadence.

## Common Commands

```bash
# Calendar
cargo run -- calendar list --days 7
cargo run -- cal ls --days 3  # alias
echo '{"subject":"1:1","start":"2025-01-15T10:00:00+01:00","end":"2025-01-15T10:30:00+01:00"}' | cargo run -- calendar create
cargo run -- agenda  # today's visual timeline

# Mail
cargo run -- mail list --folder inbox --limit 10
cargo run -- mail get --id "<message-id>"
echo '{"to":["user@example.com"],"subject":"Hello","body":"Hi"}' | cargo run -- mail send

# Contacts
cargo run -- contacts list --limit 20
echo '{"display_name":"John Doe","email":"john@example.com"}' | cargo run -- contacts create

# Free slots
cargo run -- free --weeks 2 --duration 60
```

Add `--json` for machine-readable output. Use `--account` to target another mailbox. Ensure the Python service is running before invoking the CLI.
