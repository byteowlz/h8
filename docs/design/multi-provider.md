# h8 Multi-Provider Architecture

Status: approved design, implementation in progress on `feat/multi-provider`.

## Vision

h8 is the human *and* agent interface for mail and calendars: a terminal-native,
protocol-agnostic Thunderbird-equivalent that is ergonomic for AI agents (stable JSON,
capability introspection, scoped API keys) and for humans (the existing CLI UX).

Pillars of this milestone:

1. **Provider abstraction** — EWS stays, Google Workspace is added, Microsoft Graph
   follows. One `Backend` seam in the Python service; the Rust client is untouched
   by provider choice.
2. **Integrated OAuth** — the `oama` dependency (and all GPG/pinentry machinery) is
   removed. MSAL and google-auth run in-process; tokens live in the OS keyring with a
   0600 file fallback.
3. **Service authentication + scoped access** — the FastAPI server requires bearer
   API keys. Each key carries scopes (`mail:read`, `calendar:write`, ...) and optional
   account restrictions, so different agents get different permissions.

Non-negotiable compatibility rule: **the JSON response shapes of all existing
endpoints do not change.** New backends adapt to the current EWS-derived shapes
(`id`, `changekey` (nullable), ISO-8601 datetimes with offset, etc.). The Rust client
keeps working unmodified except for the auth token and new `h8 auth` commands.

## 1. Account registry (config)

`~/.config/h8/config.toml` gains an `[accounts.*]` table. The legacy top-level
`account = "email"` keeps working and implicitly defines an EWS/M365 account.

```toml
account = "work"                       # default account: alias or bare email (legacy)

[accounts.work]
email = "user@example.com"
provider = "ews"                       # "ews" | "google" | "graph" (graph = later epic)
# Microsoft options:
client_id = "..."                      # optional; default: built-in public client id constant
tenant = "organizations"               # MSAL authority tenant, default "organizations"

[accounts.personal]
email = "someone@gmail.com"
provider = "google"
client_id = "....apps.googleusercontent.com"   # required for google
client_secret = "..."                          # installed-app secret (not confidential)
```

Resolution rules (`h8/accounts.py`):
- An incoming `account` query param may be an **alias** (`work`), or an **email**.
- Email lookup: match `accounts.*.email`; unmatched email → implicit EWS account
  (legacy behavior, so `--account someone.else@corp.com` for delegate access still works).
- `resolve_account(ref: str | None) -> AccountConfig` where
  `AccountConfig = dataclass(alias, email, provider, client_id, tenant, extra: dict)`.
- No `ref` → the configured default.

## 2. Provider abstraction (`h8-service/h8/providers/`)

Files:
- `providers/base.py` — ABCs, capability constants, exceptions. **Owned by Phase 1 seam agent.**
- `providers/registry.py` — `get_backend(account_ref: str | None) -> Backend`; caches
  per (alias) instance; replaces `auth.get_account()` at all ~50 route call sites.
- `providers/ews/__init__.py` — `EwsBackend`, a thin adapter that **delegates to the
  existing module functions** in `h8/mail.py`, `h8/calendar.py`, etc. The existing
  modules are NOT moved or rewritten in this milestone; they become the EWS
  implementation detail.
- `providers/google/` — Google implementation (Phase 2). `providers/google/__init__.py`
  is stubbed in Phase 1 (registered in the registry, raising `BackendNotSupported`)
  so Phase-2 agents only fill in module files without touching the registry.

### Backend interface

`Backend` is composed of per-domain ABCs so backends can partially implement:

```python
class Backend(ABC):
    account: AccountConfig
    provider: str                       # "ews" | "google" | "graph"
    capabilities: frozenset[str]

    def refresh(self) -> None: ...      # drop cached credentials/session, re-auth
```

Domain mixins (method names/signatures mirror the current module functions that the
FastAPI routes call today — the seam agent derives the exact list from
`service/__init__.py` call sites and freezes it in `base.py`; Phase-2 agents treat
`base.py` as the authoritative contract):

- `MailBackend` — list/get/search/send/draft/reply/forward/move/delete/mark/
  attachments/folders/empty-folder
- `CalendarBackend` — list(view)/get/create/update/delete/cancel/rsvp/search,
  meeting-request listing
- `ContactsBackend` — list/get/create/update/delete/search
- `AvailabilityBackend` — own busy times, others' free-busy (`get_free_busy(emails,
  start, end) -> dict[email, list[busy_interval]]`)
- `DirectoryBackend` — GAL search / resolve
- `SettingsBackend` — inbox rules CRUD, OOF get/set

### Capabilities

String constants in `base.py`:

```
CAP_MAIL, CAP_MAIL_SEND, CAP_MAIL_FOLDERS, CAP_MAIL_SCHEDULED_SEND,
CAP_CALENDAR, CAP_CALENDAR_MEETINGS, CAP_CONTACTS, CAP_FREEBUSY_SELF,
CAP_FREEBUSY_OTHERS, CAP_GAL, CAP_RESOURCES, CAP_RULES, CAP_OOF, CAP_BOOKINGS
```

EWS advertises all. Google (this milestone): `CAP_MAIL, CAP_MAIL_SEND,
CAP_MAIL_FOLDERS, CAP_CALENDAR, CAP_CALENDAR_MEETINGS, CAP_CONTACTS,
CAP_FREEBUSY_SELF, CAP_FREEBUSY_OTHERS, CAP_OOF`.

Routes check capability and return HTTP **501** `{"detail": "not supported by provider
'google' for account 'personal'", "missing_capability": "..."}` when unsupported.
New endpoint: `GET /capabilities?account=` → `{"account", "provider", "capabilities": [...]}`
(agents use this to introspect).

### Exceptions (`base.py`)

```python
class BackendError(Exception): ...
class BackendAuthError(BackendError): ...        # triggers refresh+retry once
class BackendBusyError(BackendError): retry_after: float | None
class BackendNotSupported(BackendError): capability: str
```

`safe_call_with_retry` in `service/__init__.py` is rewritten against these (the EWS
adapter translates `exchangelib.errors.UnauthorizedError` → `BackendAuthError`,
`ErrorServerBusy` → `BackendBusyError`). The `primary_smtp_address` duck-typing hack
is removed; retry calls `backend.refresh()` then re-invokes.

### Event-loop hygiene

`get_backend()` construction and any token acquisition MUST NOT block the event loop:
routes call it inside the existing threadpool wrappers (`safe_call*`), never directly
in the coroutine body.

## 3. OAuth subsystem (`h8-service/h8/oauth/`) — replaces oama

Files (all new; owned by Phase 1 oauth agent):
- `oauth/store.py` — `TokenStore`: tries `keyring` (service name `h8`), falls back to
  `$XDG_STATE_HOME/h8/tokens.json` chmod 0600. API: `get(key) -> str | None`,
  `set(key, value)`, `delete(key)`. Values are opaque strings (serialized caches).
- `oauth/microsoft.py` — MSAL `PublicClientApplication` per (client_id, tenant), with
  `SerializableTokenCache` persisted through `TokenStore` under `ms:{email}`.
  - `get_ms_token(acct: AccountConfig, resource: Literal["ews","graph"]) -> AccessToken`
    where `AccessToken = dataclass(token: str, expires_at: float)`.
    Scopes: ews → `https://outlook.office365.com/EWS.AccessAsUser.All`;
    graph → `Mail.ReadWrite Mail.Send Calendars.ReadWrite Contacts.ReadWrite
    MailboxSettings.ReadWrite People.Read User.ReadBasic.All offline_access`.
    Silent first (`acquire_token_silent`), else raise `LoginRequired` (defined in
    `oauth/__init__.py`).
  - Interactive: `start_device_login(acct) -> DeviceLogin(session_id, verification_url,
    user_code, expires_at)` + `poll_device_login(session_id) -> "pending"|"done"|"error:..."`.
    Sessions held in an in-memory dict.
- `oauth/google.py` — google-auth. Credentials JSON persisted under `google:{email}`.
  - `get_google_credentials(acct) -> google.oauth2.credentials.Credentials`
    (auto-refreshes and re-persists; raises `LoginRequired` when absent/revoked).
  - Scopes: `https://www.googleapis.com/auth/gmail.modify`,
    `.../auth/gmail.settings.basic`, `.../auth/calendar`, `.../auth/contacts`.
  - Interactive: loopback flow (`InstalledAppFlow.run_local_server`) when a browser is
    reachable; headless fallback: generate auth URL with `redirect_uri=http://localhost:1`
    style loopback + manual paste of the full redirect URL (`start_url_login` /
    `finish_url_login(session_id, redirect_url)`).
  - README must document the "publish OAuth app to Production or refresh tokens die
    in 7 days" Google trap.
- `oauth/__init__.py` — facade: `login_status(acct) -> dict`, `logout(acct)`,
  `LoginRequired` exception.

Dependencies added to `pyproject.toml`: `msal`, `google-auth`, `google-auth-oauthlib`,
`google-api-python-client`, `keyring`.

### Auth endpoints (service, Phase 2; admin-scoped once Epic D lands)

- `GET  /auth/accounts` → `[{alias, email, provider, logged_in, expires_at}]`
- `POST /auth/login` `{account}` → device-code: `{flow:"device_code", session_id,
  verification_url, user_code}`; google-headless: `{flow:"auth_url", session_id, auth_url}`
- `POST /auth/login/{session_id}/finish` `{redirect_url?}` → for google URL-paste flow
- `GET  /auth/login/{session_id}` → `{status: "pending"|"done"|"error", detail?}`
- `POST /auth/logout` `{account}`

CLI: `h8-service auth login|status|logout [account]` (server host), and Rust
`h8 auth login|status|logout [account]` driving the endpoints above (prints the
device code / URL, polls until done).

### oama removal

Delete from `auth.py`: `is_oama_installed`, `install_oama`, `ensure_oama`,
`get_oama_platform_suffix`, `get_latest_oama_version`, `renew_token`, `get_token`,
`ensure_gpg_headless`, `_ensure_config_line`. Delete the `start_service` oama check in
`h8-cli/src/main.rs`. Rewrite README (remove oama + both GPG sections; add OAuth setup
for Microsoft and Google). `CachedAccount`'s fixed 55-min lifetime is replaced by the
real `expires_at` from the token layer.

## 4. Google backend mapping decisions

- **IDs**: Gmail message id / Calendar event id / People resourceName are returned in
  the existing `id` field; `changekey` is `null` (People API `etag` goes in `changekey`
  since contacts updates need it).
- **Folders ↔ labels**: `inbox→INBOX`, `sent→SENT`, `drafts` (drafts API), `trash→TRASH`,
  `junk→SPAM`, `archive` = remove `INBOX` label. Arbitrary folder names map to
  user labels (create on demand). Folder listing lists labels.
- **Search**: h8 query syntax (`from:`, `subject:`, `body:`, `|` for OR) translates to
  Gmail `q=` syntax (native `from:`/`subject:`; `body:` terms become bare terms;
  `|` → `OR`).
- **Send/draft**: build RFC 2822 MIME (`email.message.EmailMessage`), base64url raw;
  attachments as MIME parts. Reply/forward set `threadId` + `In-Reply-To`/`References`.
- **Calendar**: `events.list(timeMin,timeMax,singleEvents=true)` for views;
  attendees + `sendUpdates=all` for meetings; RSVP = patch own attendee responseStatus.
  Free/busy (self and others): `freebusy.query`.
- **Contacts**: People API `people.connections.list` with
  `personFields=names,emailAddresses,phoneNumbers,organizations`; updates need `etag`.
- **OOF**: Gmail `users.settings.updateVacation` (no internal/external split — the
  external fields of the h8 OOF shape mirror the single response).
- Unsupported (return 501 via capabilities): GAL/resolve, resources, inbox rules,
  scheduled send, Bookings parsing.

## 5. Service authentication & scoped access (Epic D)

### Key model

`$XDG_STATE_HOME/h8/keys.json` (0600):

```json
{"keys": [{"id": "k_x7ab", "name": "claude-mail-reader",
  "hash": "<sha256 hex of token>", "scopes": ["mail:read", "calendar:read"],
  "accounts": null, "created_at": "...", "last_used_at": "...", "disabled": false}]}
```

Token format shown once at creation: `h8k_<43 chars urlsafe random (32 bytes)>`.
Lookup = constant-time compare of sha256.

### Scope grammar

`<resource>:<action>`; resource ∈ `mail, calendar, contacts, addr, resources, trip,
rules, oof, unsubscribe, auth, keys, admin`; action ∈ `read, write, send`; `*`
wildcard on either side. Examples: `mail:read`, `mail:send`, `calendar:*`, `*:read`,
`*:*`. Deny is the default — a key only does what its scopes grant. Optional
`accounts: ["work"]` restricts which account aliases/emails the key may target
(server 403s other `account=` values).

Route mapping principles: GET endpoints → `<resource>:read`; mutating mail endpoints →
`mail:write` except send/reply/forward-send → `mail:send`; calendar create/update/
delete/rsvp → `calendar:write`; `/auth/*`, `/keys/*`, cache admin, `/refresh` →
`admin:*` (or `keys:*` for key CRUD). `/health` and `/capabilities` are unauthenticated.

### Bootstrap & enforcement

- On startup, if `keys.json` has no enabled key, generate a **root key**
  (`scopes: ["*:*"]`, name `root`) and write the raw token to
  `$XDG_STATE_HOME/h8/client.key` (0600). The Rust client reads, in order:
  `H8_TOKEN` env → `service_token` in config.toml → `$XDG_STATE_HOME/h8/client.key`.
  So auth is on by default with zero UX friction on a single-user box.
- FastAPI: `Security` dependency `require_scope(scope)` applied per-route; missing
  header → 401; bad token → 401; insufficient scope → 403 `{"detail", "required_scope"}`.
  Escape hatch for debugging: `H8_SERVICE_NO_AUTH=1`.
- Key management: `h8-service keys create --name N --scopes s1,s2 [--accounts a,b]`,
  `keys list`, `keys revoke <id>`; HTTP: `GET/POST /keys`, `DELETE /keys/{id}`
  (scope `keys:*`). Rust: `h8 keys create|list|revoke` proxying HTTP.
- **Audit log**: `$XDG_STATE_HOME/h8/audit.jsonl`, one line per authenticated request:
  `{ts, key_id, key_name, method, path, account, status, duration_ms}`. Toggle
  `H8_SERVICE_AUDIT=0` to disable.

### Hardening (same epic)

- Host-header allowlist (`localhost`, `127.0.0.1`, `[::1]`, configured hostname) → 400
  otherwise (DNS-rebinding guard).
- Bearer requirement kills the multipart CSRF vector on `/mail/send-files`.
- SSRF guard in `unsubscribe.py`: resolve target host before request; block loopback,
  RFC-1918, link-local (169.254.0.0/16 incl. metadata IP), and re-check on redirects
  (`follow_redirects=False` + manual loop, max 3 hops).

## 6. Rust client changes (h8-client)

- `ServiceClient`: add optional bearer token, sent as `Authorization: Bearer ...` on
  every request. Token discovery order as above. On 401 → actionable error message
  ("run h8-service to generate a client key / set H8_TOKEN").
- New commands: `h8 auth login|status|logout [account]` (device-code/URL flow via
  `/auth/*`, poll loop with printed instructions), `h8 keys create|list|revoke`.
- 403 responses surface `required_scope` in the error message (agent ergonomics).
- Remove the oama check in `start_service`.

## 7. Phasing & file ownership (implementation)

| Phase | Agent | Owns (exclusive) |
|---|---|---|
| 1 | seam (opus) | `h8/providers/base.py`, `registry.py`, `ews/`, `google/__init__.py` stub, `h8/accounts.py`, `h8/config.py`, `h8/auth.py`, `h8/service/__init__.py`, contract tests |
| 1 | oauth (opus) | `h8/oauth/**`, `pyproject.toml` |
| 1 | rust (sonnet) | `h8-client/**` (token plumbing, `h8 auth`/`h8 keys` commands against this spec) |
| 2 | ms-integration (opus) | `h8/auth.py` slimming, EWS token source → oauth, oama/GPG deletion, `/auth/*` endpoints, `h8-service auth` CLI, README auth sections |
| 2 | gmail (opus) | `h8/providers/google/client.py`, `google/mail.py` |
| 2 | gcal (opus) | `h8/providers/google/calendar.py` (incl. freebusy) |
| 2 | gcontacts (sonnet) | `h8/providers/google/contacts.py`, `google/settings.py` (OOF), wiring them into `google/__init__.py` |
| 3 | service-auth (opus) | `h8/security.py`, keys store/CLI, route scope wiring, audit middleware, hardening, `/keys` endpoints |
| 4 | verify/fix + docs (mixed) | test runs, integration fixes, README/AGENTS.md |

`providers/google/__init__.py` in Phase 2: the gmail agent creates `client.py`
(credential → service builders: `gmail_service(acct)`, `calendar_service(acct)`,
`people_service(acct)`) and the `GoogleBackend` class skeleton; gcal/gcontacts agents
only add their own modules and the mixin wiring lines noted as TODO markers left by
the gmail agent.
