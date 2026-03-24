![banner](banner.png)

# h8

Rust CLI for MS365 Exchange Web Services (EWS) covering calendar, mail, contacts, free-slot search, resource management, booking, and business trip planning. Works when Graph or IMAP are blocked but EWS is available.

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

### Headless Server Deployment

On headless Linux servers (no display), GPG's pinentry can hang because it tries to open a GUI/TUI dialog. h8-service detects headless environments and automatically configures GPG loopback pinentry on startup, but you still need a GPG key that works non-interactively.

**Option A: Pre-cache your existing key's passphrase**

If your GPG key has a passphrase, extend the agent cache and unlock once after each reboot:

```bash
# Extend cache to 24 hours
echo "default-cache-ttl 86400" >> ~/.gnupg/gpg-agent.conf
echo "max-cache-ttl 86400" >> ~/.gnupg/gpg-agent.conf
gpgconf --kill gpg-agent

# Unlock the key (run once after reboot, e.g. in a systemd ExecStartPre)
echo "YOUR_PASSPHRASE" | gpg --batch --passphrase-fd 0 --pinentry-mode loopback --sign /dev/null
```

**Option B: Use a passphrase-less GPG key**

> **Security note:** A passphrase-less key means anyone with access to the server's filesystem can decrypt the stored OAuth tokens. Only use this on servers with restricted access and appropriate filesystem permissions. Consider disk encryption as an additional layer of protection.

1. Generate a key without a passphrase:

```bash
gpg --batch --gen-key <<EOF
%no-protection
Key-Type: RSA
Key-Length: 2048
Name-Real: h8-service
Name-Email: h8-service@localhost
Expire-Date: 0
%commit
EOF
```

2. Update `~/.config/oama/config.yaml` to use the new key:

```yaml
encryption:
  tag: GPG
  contents: h8-service@localhost
```

3. Re-authorize oama (tokens must be re-encrypted with the new key):

```bash
oama authorize microsoft/your.email@example.com
```

4. Verify it works non-interactively:

```bash
oama access your.email@example.com
```

## Architecture

- A Python service (FastAPI) talks to EWS via `exchangelib`, handles geocoding and routing via public APIs (Nominatim, OSRM), and caches data locally.
- The Rust CLI calls the local service for all calendar/mail/contact/resource/routing operations.

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
