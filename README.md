![banner](banner.png)

# h8

Rust CLI for MS365 Exchange Web Services (EWS) covering calendar, mail, contacts, free-slot search, resource management, booking, and business trip planning. Works when Graph or IMAP are blocked but EWS is available.

## Requirements

- Rust stable toolchain
- Python 3.12+ with `uv`

OAuth is handled in-process: MSAL (Microsoft) and google-auth (Google) acquire
and refresh tokens directly, storing them in the OS keyring with a 0600 file
fallback at `$XDG_STATE_HOME/h8/tokens.json`. There is no external token daemon
and no GPG/pinentry setup.

Accounts are declared in `config.toml` under `[accounts.*]` and each names a
`provider` (`ews`, `google`, or `graph`). You sign in once per account with
`h8 auth login <account>`; tokens refresh silently afterwards.

### Microsoft 365 setup (provider `ews`)

Microsoft accounts authenticate through an Azure AD (Entra) app registration and
the OAuth device-code flow.

1. In the [Azure portal](https://portal.azure.com) go to **Entra ID -> App
   registrations -> New registration**. Give it a name and register it as a
   public client.
2. Under **Authentication**, enable **Allow public client flows** (required for
   device code), and add the **Mobile and desktop applications** platform.
3. Under **API permissions**, add these delegated permissions and grant consent:
   - Office 365 Exchange Online: `EWS.AccessAsUser.All`
   - Microsoft Graph: `Mail.ReadWrite`, `Mail.Send`, `Calendars.ReadWrite`,
     `Contacts.ReadWrite`, `MailboxSettings.ReadWrite`, `People.Read`,
     `User.ReadBasic.All`
4. Copy the **Application (client) ID** and put it in the account config:

```toml
account = "work"                       # default account (alias or bare email)

[accounts.work]
email = "you@example.com"
provider = "ews"
client_id = "00000000-0000-0000-0000-000000000000"   # your app registration id
tenant = "organizations"                              # or your tenant GUID/domain
```

5. Sign in (opens the device-code flow -- visit the URL and enter the code):

```bash
h8 auth login work
```

### Google Workspace setup (provider `google`)

Google accounts authenticate through a Google Cloud OAuth client and either a
loopback browser flow or a headless URL-paste flow.

1. In the [Google Cloud Console](https://console.cloud.google.com) create (or
   pick) a project and enable the **Gmail API**, **Google Calendar API**, and
   **People API**.
2. Configure the **OAuth consent screen**. It **must be published to "In
   Production"** -- while it stays in "Testing", Google issues refresh tokens
   that **expire after 7 days**, which silently breaks background refresh. For
   personal use the "unverified app" warning on the consent screen is
   click-through-able; publishing to Production is still required for durable
   refresh tokens.
3. Create an **OAuth client ID** of type **Desktop app**. Copy the client id and
   client secret (the installed-app secret is not confidential) into the config:

```toml
[accounts.personal]
email = "you@gmail.com"
provider = "google"
client_id = "xxxx.apps.googleusercontent.com"
client_secret = "xxxx"
```

4. Sign in:

```bash
h8 auth login personal
```

On a machine with a browser, this opens a loopback authorization page. On a
headless host, h8 prints an authorization URL: open it elsewhere, approve
access, then paste the full `http://localhost/...` redirect URL back into the
prompt to complete the login.

### Managing logins

```bash
h8 auth status [account]     # show login state and token expiry
h8 auth login  <account>     # sign in (device code for MS, URL flow for Google)
h8 auth logout <account>     # delete stored credentials

# The same commands are available server-side without the HTTP service running:
h8-service auth status
h8-service auth login work
h8-service auth logout work
```

## Architecture

- A Python service (FastAPI) exposes a provider-agnostic backend seam: EWS today
  (via `exchangelib`), with Google Workspace and Microsoft Graph backends behind
  the same interface. It also handles geocoding and routing via public APIs
  (Nominatim, OSRM) and caches data locally.
- Each account is resolved to a provider backend; capabilities are introspectable
  via `GET /capabilities` and unsupported operations return HTTP 501.
- OAuth (MSAL and google-auth) runs in-process; tokens live in the OS keyring
  with a 0600 file fallback.
- The Rust CLI calls the local service for all calendar/mail/contact/resource/
  routing operations, and drives login via `h8 auth`.

## Setup

```bash
# Install Rust CLI and Python deps
just install

# Install the Python service globally (enables `h8-service start` from anywhere)
cd ~/path/to/h8
uv tool install -e .

# Start the Python service
h8-service start   # runs in background, logs to ~/.local/state/h8/service.log

# Check status / stop
h8-service status
h8-service stop
h8-service restart
```

## Configuration

Default path: `$XDG_CONFIG_HOME/h8/config.toml` (or `~/.config/h8/config.toml`). Overrides: local `./config.toml`, env (`H8__...`), then `--config`. CLI flags take precedence.

```toml
account = "your.email@example.com"
timezone = "Europe/Berlin"

[people]
alice = "alice.smith@example.com"
bob = "bob.jones@example.com"

[resources.cars]
car1 = { email = "resource.car1@example.com", desc = "Toyota Camry" }
car2 = "resource.car2@example.com"

[resources.rooms]
conf-a = { email = "room.conf-a@example.com", desc = "Conference Room A" }

[trip]
default_origin = "work"
buffer_minutes = 15
transit_provider = "db"

[trip.locations.work]
address = "123 Main St, City"
lat = 51.5074
lon = -0.1278
station = "London Paddington"
```

See `examples/config.toml` for a full template with all options.

## Logging

- CLI: `RUST_LOG=debug` (or `--debug`/`--trace`) for verbose output.
- Service: `H8_SERVICE_LOGLEVEL` (`INFO`/`DEBUG`), `H8_SERVICE_CACHE_TTL` (seconds), and `H8_SERVICE_REFRESH_SECONDS` control caching/refresh.

## Usage

### Calendar

```bash
h8 agenda                              # today's timeline view
h8 cal show today                      # today's events
h8 cal show tomorrow                   # tomorrow's events
h8 cal show friday                     # events on Friday
h8 cal show "next week"                # next week's events
h8 cal show kw30                       # calendar week 30
h8 cal add friday 2pm Team Sync        # natural language event creation
h8 cal add 'tomorrow 10am-11am Review' # with time range
h8 cal delete <id>                     # delete event
h8 cal search "standup"                # search events
```

### Mail

```bash
h8 mail list                           # inbox, last 20
h8 mail list today                     # today's emails
h8 mail list -u                        # unread only
h8 mail list -f sent -l 50             # sent folder, 50 items
h8 mail read <id>                      # view in pager
h8 mail compose                        # opens editor, saves draft
h8 mail send <draft-id>                # send a draft
h8 mail send --to X --subject Y --attach ./report.pdf   # send with attachment
h8 mail reply <id>                     # reply to sender
h8 mail reply <id> --all               # reply all
h8 mail forward <id>                   # forward
h8 mail search "meeting notes"         # search
h8 mail attachments <id>               # list attachments
h8 mail attachments <id> -d 0 -o ./    # download first attachment
```

### Contacts

```bash
h8 contacts list                       # list contacts
h8 contacts list -s "alice"            # search
h8 contacts get --id <id>              # view details
h8 contacts update --id <id> --phone "+1 555 1234"
```

### People

```bash
h8 ppl agenda alice                    # view alice's calendar
h8 ppl free alice --weeks 2            # find alice's free slots
h8 ppl common alice bob --weeks 2      # common free slots
h8 ppl schedule alice bob -w 2 --json  # list schedulable slots
```

### Address Book (GAL)

```bash
h8 addr search "john smith"            # search Global Address List
h8 addr resolve meeting-room           # resolve name via EWS
```

### Resources

Manage shared bookable resources (rooms, cars, equipment) defined in `[resources.*]` config sections.

```bash
h8 resource list                       # list all resource groups
h8 resource free cars tomorrow         # check car availability
h8 resource free rooms friday 14-16    # room availability in time window
h8 resource agenda cars monday         # view car bookings
h8 resource setup rooms                # interactive: search GAL, add resources
h8 resource remove cars old-car        # remove a resource alias
```

Natural language queries:

```bash
h8 which cars are free                 # today
h8 which rooms are free friday 13-15   # specific window
h8 is the bmw free tomorrow            # single resource check
```

### Booking

Book resources interactively or programmatically:

```bash
h8 book room today 12-14               # interactive: pick from available rooms
h8 book car tomorrow 9-12              # interactive: pick from available cars
h8 book room friday 14-16 --select conf-a --subject "Team Sync"  # direct booking
h8 book room today 12-14 --json        # JSON output of availability
```

### Trip Planning

Plan business trips with automatic travel time calculation, car booking, and calendar creation. Uses free global routing services (OSRM for driving, Nominatim for geocoding).

```bash
# Plan a trip (shows timeline)
h8 trip Berlin friday 9-12 --car
h8 trip Munich tomorrow 14-16 --transit
h8 trip "New York" monday 9-17 --car   # works worldwide

# From a different origin
h8 trip Berlin friday 9-12 --car --from home

# Book a car for the trip
h8 trip Berlin friday 9-12 --car --book

# Create calendar events (travel-to, meeting, travel-back)
h8 trip Berlin friday 9-12 --car --create --subject "Client Meeting"

# Programmatic / JSON output
h8 trip Berlin friday 9-12 --car --json

# SAP-compatible export
h8 trip Berlin friday 9-12 --car --sap --json
```

### Free Slots

```bash
h8 free                                # your free slots this week
h8 free -w 2 -d 60                     # 2 weeks, 60-min slots
h8 ppl free alice                      # someone's free slots
h8 ppl common alice bob                # common free time
```

### Availability

```bash
h8 free                                # your free slots
h8 ppl agenda alice                    # someone's calendar
h8 ppl free alice                      # their free slots
h8 ppl common alice bob                # common free time
```

All commands support `--json` and `--yaml` for machine-readable output. Use `--account` to target another mailbox.

## License

MIT
