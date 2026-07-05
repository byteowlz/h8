# h8 (Rust)

Rust CLI for calendar, mail, contacts, and free-slot discovery against a
multi-provider backend (Microsoft 365 Exchange Web Services and Google
Workspace today). OAuth (MSAL / google-auth) runs in-process in the Python
service -- there is no external token daemon such as `oama`.

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

## Authentication

`h8-service` requires a bearer API key by default. The client discovers a token in
this order (first match wins): the `H8_TOKEN` environment variable, `service_token`
in `config.toml`, then `$XDG_STATE_HOME/h8/client.key` (default
`~/.local/state/h8/client.key`, trimmed). On first run the service writes a root
key to that file, so a single-user setup works with zero configuration. If no
token is found anywhere, requests are sent without an `Authorization` header
(useful when the service runs with `H8_SERVICE_NO_AUTH=1`).

Manage account logins (Microsoft/Google OAuth) via `h8 auth`:

```bash
h8 auth status              # table of configured accounts and login state
h8 auth login                # log in the default account (device-code or browser-URL flow)
h8 auth login personal       # log in a specific account alias/email
h8 auth logout personal      # discard stored credentials for an account
```

Manage scoped API keys via `h8 keys` (requires an existing key with `keys:*`/`admin:*` scope):

```bash
h8 keys list
h8 keys create --name claude-mail-reader --scopes mail:read,calendar:read
h8 keys create --name agent --scopes "mail:*" --accounts work,personal
h8 keys revoke k_x7ab
```

The raw token from `h8 keys create` is only ever shown once in the command's output
-- store it immediately (e.g. as `H8_TOKEN` for the agent using it).

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
