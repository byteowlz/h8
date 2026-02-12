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
h8-service/           Python FastAPI (exchangelib for EWS)
  h8/contacts.py      Contact CRUD functions
  h8/mail.py          Mail operations
  h8/calendar.py      Calendar operations
  h8/service/__init__.py  FastAPI routes (add endpoints here)

h8-client/            Rust workspace
  h8-core/src/
    service.rs        HTTP client (add client methods here)
    types.rs          Shared types
  h8-cli/src/main.rs  CLI commands (commands + handlers)
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

## CLI Patterns

**Short IDs**: Mail/calendar use `adjective-noun` IDs (e.g., `cold-lamp`). Resolve via:
```rust
let remote_id = id_gen.resolve(&args.id)?.unwrap_or_else(|| args.id.clone());
```

**Output**: Use `emit_output(&ctx.common, &result)?;` for JSON/YAML/table output.

**Service calls**: Always use `client.method().map_err(|e| anyhow!("{e}"))?`

## CLI Commands Reference

| Command | Description |
|---------|-------------|
| `h8 mail list [when]` | List messages (supports: today, monday, jan 15) |
| `h8 mail read <id>` | View message in pager |
| `h8 mail compose` | Create draft in editor |
| `h8 mail reply <id> [--all]` | Reply to message |
| `h8 mail send <id>` | Send draft |
| `h8 cal show [when]` | Show events (today, tomorrow, friday, kw30, next week) |
| `h8 cal add "fri 2pm Meeting"` | Natural language event creation |
| `h8 contacts list [-s search]` | List/search contacts |
| `h8 contacts update --id <id> --phone <phone>` | Update contact |
| `h8 agenda` | Today's calendar |
| `h8 ppl schedule A B -w N --json` | List common free slots (step 1) |
| `h8 ppl schedule A B --slot N -s "Subj" -m 45` | Book a slot (step 2) |

## Issue Tracking
```bash
trx list                    # Show open issues
trx create "title" -t feature -p 1  # P1=high, P2=medium, P3=low
trx close <id> -r "reason"
trx sync                    # Commit .trx/
```

## Code Style
- **Python**: Type hints, docstrings, grouped imports (stdlib/external/local)
- **Rust**: `thiserror` for lib errors, `anyhow` for CLI, doc comments on public items
- **No emojis** in code/docs/commits
