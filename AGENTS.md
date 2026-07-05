# AGENTS.md

## Build/Test
```bash
just install          # uv sync + cargo install
cargo check           # fast Rust check (h8-client/)
cargo test            # Rust tests
uv run pytest tests/  # Python tests
```

## Architecture
```
h8-service/           Python FastAPI (routing APIs; EWS + Google Workspace backends)
  h8/providers/       Provider abstraction -- the backend seam
    base.py           Backend contract: domain mixins (Mail/Calendar/Contacts/
                       Availability/Directory/Settings), capability constants
                       (CAP_*), exceptions (BackendAuthError, BackendBusyError,
                       BackendNotSupported). FROZEN method signatures.
    registry.py       get_backend(account_ref) -> cached Backend instance;
                       provider id -> factory in _FACTORIES
    ews/__init__.py   EwsBackend: thin adapter delegating to h8/mail.py,
                       calendar.py, contacts.py, etc. (unchanged EWS logic)
    google/           GoogleBackend: Gmail, Calendar, People API
                       (client.py, mail.py, calendar.py, contacts.py, settings.py)
  h8/oauth/           Integrated OAuth (no external daemon, no GPG)
    microsoft.py      MSAL device-code login + silent token refresh
    google.py         google-auth loopback/URL-paste login + refresh
    store.py          TokenStore: OS keyring, falls back to
                       $XDG_STATE_HOME/h8/tokens.json (0600)
  h8/accounts.py      [accounts.*] config resolution: alias/email -> AccountConfig
  h8/security.py      Service auth: h8k_ bearer keys, scope grammar, KeyStore
                       ($XDG_STATE_HOME/h8/keys.json), audit log
  h8/contacts.py      Contact CRUD functions (EWS implementation detail)
  h8/mail.py          Mail operations (EWS implementation detail)
  h8/calendar.py      Calendar operations (EWS implementation detail)
  h8/resolve.py       EWS ResolveNames for GAL search + email validation
  h8/resources.py     Resource availability via EWS
  h8/routing.py       Geocoding (Nominatim), car routing (OSRM), transit (DB HAFAS)
  h8/unsubscribe.py   Bulk unsubscribe: link extraction, HTTP visiting (SSRF-guarded)
  h8/service/__init__.py  FastAPI routes + auth wiring (add endpoints here)

h8-client/            Rust workspace
  h8-core/src/
    config.rs         Config types: AppConfig, TripConfig, Location, ResourceEntry, etc.
    service.rs        HTTP client (add client methods here); bearer token discovery
    types.rs          Shared types
  h8-cli/src/main.rs  CLI commands (commands + handlers), incl. `auth`/`keys`
```

## Adding a Feature (example: contacts update)

1. **Python function** (`h8/contacts.py`):
```python
def update_contact(account: Account, item_id: str, updates: dict) -> dict:
    """Update contact fields. Returns updated contact dict."""
```

2. **API endpoint** (`h8/service/__init__.py`):
```python
class ContactUpdate(BaseModel):
    display_name: Optional[str] = None
    # ... fields

@app.put("/contacts/{item_id}")
async def contacts_update(item_id: str, payload: ContactUpdate, account: Optional[str] = None):
    return await safe_call_with_retry(contacts.update_contact, email, acct, item_id, update_data)
```

3. **Rust client** (`h8-core/src/service.rs`):
```rust
pub fn contacts_update(&self, account: &str, id: &str, updates: Value) -> Result<Value> {
    self.put_json(&format!("/contacts/{}?account={}", id, account), updates)
}
```

4. **CLI command** (`h8-cli/src/main.rs`):
   - Add variant to enum: `Update(ContactsUpdateArgs)`
   - Add args struct: `struct ContactsUpdateArgs { ... }`
   - Add match arm in handler: `ContactsCommand::Update(args) => { ... }`

## Adding a Provider Backend

1. Implement only the domain mixins you support (`MailBackend`, `CalendarBackend`,
   `ContactsBackend`, `AvailabilityBackend`, `DirectoryBackend`, `SettingsBackend`)
   in `h8/providers/<name>/`, against the frozen method names/signatures in
   `h8/providers/base.py`. Unimplemented methods raise `NotImplementedError` by
   default -- that's fine as long as the capability isn't advertised.
2. Set `capabilities: frozenset[CAP_*]` on the backend class to exactly what you
   implement. Routes gate on capability before calling the method and return
   HTTP 501 (`missing_capability`) otherwise -- never let an unadvertised method
   get called.
3. Register the provider id -> factory mapping in
   `h8/providers/registry.py` (`_FACTORIES`).
4. Response shapes are FROZEN to the existing EWS-derived JSON (`id`, `changekey`
   nullable, ISO-8601 datetimes with offset) -- adapt to that shape, don't
   change existing endpoint contracts. See `docs/design/multi-provider.md`
   section 4 for the Google mapping as a worked example.

## Auth Model (for agents)

- Every request needs `Authorization: Bearer h8k_...` (401 without one, except
  `/health` and `/capabilities`). A root key (`*:*` scope) is auto-generated on
  first run at `~/.local/state/h8/client.key`.
- Keys carry scopes `<resource>:<action>` (`mail:read`, `calendar:write`, ...),
  with `*` wildcard on either side (`mail:*`, `*:read`, `*:*`). Deny by default.
  Keys may also carry an `accounts` restriction list.
- Insufficient scope -> 403 with `required_scope` in the response body (surfaced
  by the Rust client's error message).
- Use `GET /capabilities?account=` (unauthenticated) to introspect what a given
  account's provider supports before calling an endpoint that might 501.
- Manage keys with `h8 keys` / `h8-service keys` (see the commands table below).

## CLI Patterns

**Short IDs**: Mail/calendar use `adjective-noun` IDs (e.g., `cold-lamp`). Resolve via:
```rust
let remote_id = id_gen.resolve(&args.id)?.unwrap_or_else(|| args.id.clone());
```

**Output**: Use `emit_output(&ctx.common, &result)?;` for JSON/YAML/table output.

**Service calls**: Always use `client.method().map_err(|e| anyhow!("{e}"))?`

**Trailing var arg + flags**: Commands using `trailing_var_arg = true` (resource free/agenda,
natural language queries, trip) must strip global flags (`--json`, `--yaml`) and command-specific
flags from the captured words. Use `strip_global_flags()` for global flags. For trip, use
`parse_trip_flags()` which extracts `--car`, `--transit`, `--book`, `--create`, `--sap`, etc.

## CLI Commands Reference

| Command | Description |
|---------|-------------|
| `h8 mail list [when]` | List messages (supports: today, monday, jan 15) |
| `h8 mail read <id>` | View message in pager |
| `h8 mail compose` | Create draft in editor |
| `h8 mail send --draft --to X --subject Y --body Z` | Create draft non-interactively (agent-safe) |
| `h8 mail reply <id> [--all]` | Reply to message |
| `h8 mail send <id>` | Send draft |
| `h8 mail send --to X --subject Y --body Z` | Send directly (no draft) |
| `h8 mail send --to X --subject Y --attach F` | Send with attachment (repeat `--attach`) |
| `h8 mail unsubscribe [OPTIONS]` | Bulk unsubscribe from marketing emails |
| `h8 mail search "query" [-d N] [--from/--to]` | Search mail (OR via `\|`, field: `from:`, `subject:`, `body:`) |
| `h8 cal show [when] [--from/--to]` | Show events (natural lang or explicit date range) |
| `h8 cal add "fri 2pm Meeting"` | Natural language event (no time = all-day) |
| `h8 cal add "Urlaub 03-30 bis 04-11"` | Multi-day event (till/until/bis/through) |
| `h8 contacts list [-s search]` | List/search contacts |
| `h8 contacts update --id <id> --phone <phone>` | Update contact |
| `h8 agenda` | Today's calendar |
| `h8 ppl schedule A B -w N --json` | List common free slots (step 1) |
| `h8 ppl schedule A B --slot N -s "Subj" -m 45` | Book a slot (step 2) |
| `h8 addr search "query"` | Search Global Address List |
| `h8 addr resolve "query"` | Resolve name via EWS ResolveNames |
| `h8 resource list` | List all resource groups and aliases |
| `h8 resource free <group> [when]` | Check resource group availability |
| `h8 resource agenda <group> [when]` | View resource group bookings |
| `h8 resource setup [group] [-q query]` | Interactive: search GAL, add resources |
| `h8 resource remove <group> <alias>` | Remove a resource alias from config |
| `h8 which <group> are free [when]` | Natural language resource query |
| `h8 is the <alias> free [when]` | Natural language single resource check |
| `h8 book <group> <when> [--select <alias> --subject <text>]` | Book a resource |
| `h8 trip <dest> <when> --car/--transit` | Plan business trip with routing |
| `h8 trip <dest> <when> --car --book` | Plan trip + book a car |
| `h8 trip <dest> <when> --car --create` | Plan trip + create calendar events |
| `h8 trip <dest> <when> --car --sap --json` | Trip plan as SAP-compatible JSON |
| `h8 auth login [account]` | Sign in (device-code for MS, browser/URL flow for Google) |
| `h8 auth status [account]` | Show login state and token expiry |
| `h8 auth logout [account]` | Delete stored credentials for an account |
| `h8 keys create --name N --scopes s1,s2 [--accounts a,b]` | Create a scoped API key (shown once) |
| `h8 keys list` / `h8 keys revoke <id>` | List / revoke API keys |
| `h8-service auth login\|status\|logout [account]` | Same as `h8 auth`, server-side, no HTTP round trip |
| `h8-service keys create\|list\|revoke ...` | Same as `h8 keys`, direct against the key store |

## Config Sections

| Section | Purpose |
|---------|---------|
| `account`, `timezone`, `service_url`, `service_token` | Core settings |
| `[accounts.<alias>]` | Named accounts: `email`, `provider` (ews/google/graph), `client_id`, `tenant`, provider-specific extras |
| `[calendar]` | Display preferences (default_view) |
| `[free_slots]` | Working hours, weekend exclusion |
| `[mail]` | Pager, editor, signature, compose settings |
| `[people]` | Name-to-email aliases for ppl commands |
| `[resources.<group>]` | Bookable resource groups (cars, rooms, etc.) |
| `[trip]` | Default origin, buffer, routing providers, country |
| `[trip.locations.<alias>]` | Named locations with coordinates and station |
| `[unsubscribe]` | Safe senders, trusted domains, rate limiting |

## Routing Providers

| Provider | Mode | Coverage | API Key |
|----------|------|----------|---------|
| OSRM | car | Worldwide | None (free) |
| Nominatim | geocoding | Worldwide | None (free) |
| DB HAFAS | transit | Germany | None (free) |
| OpenRouteService | car | Worldwide | Required |

Transit providers are pluggable -- add new ones in `h8/routing.py` and register in `route_transit()`.

## Issue Tracking
```bash
trx list                    # Show open issues
trx create "title" -t feature -p 1  # P1=high, P2=medium, P3=low
trx close <id> -r "reason"
trx sync                    # Commit .trx/
```

## Flexible Date Formats

All `--from`, `--to`, `--start`, `--end` flags accept flexible formats:
- ISO: `2026-03-11`, compact: `20260311`, US: `03/11/2026`, German: `11.03.2026`
- Relative: `"last week"`, `"past month"`, `"past 14 days"`, `"next week"`, `"next month"`
- Natural: `today`, `tomorrow`, `friday`, `"march 15"`

## Code Style
- **Python**: Type hints, docstrings, grouped imports (stdlib/external/local)
- **Rust**: `thiserror` for lib errors, `anyhow` for CLI, doc comments on public items
- **No emojis** in code/docs/commits
