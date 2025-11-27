# ms365-bridge

Lightweight Python bridge for MS365 EWS access when IMAP/Graph API are blocked.

## Problem

- Employer blocks IMAP and Graph API
- EWS works (Thunderbird client ID is whitelisted)
- Need programmatic access to email + calendar

## Solution

Python CLI/library using `exchangelib` with OAuth tokens from `oama`.

## Features

### MVP
- [ ] Fetch emails (list, read, search)
- [ ] Fetch calendar events (list by date range)
- [ ] JSON output for scripting
- [ ] Use oama for token management

### Later
- [ ] Send email
- [ ] Create/update calendar events
- [ ] Local CalDAV server (for calendar app integration)
- [ ] Watch/sync mode

## Architecture

```
oama (token) --> ms365-bridge --> EWS API
                     |
                     v
              JSON / stdout
```

## Dependencies

- `exchangelib` - EWS client
- `click` - CLI
- `oama` - OAuth token management (external)

## Usage (planned)

```bash
# List recent emails
ms365-bridge mail list --limit 10

# Read specific email
ms365-bridge mail read <id>

# List calendar events
ms365-bridge cal list --days 7

# JSON output
ms365-bridge cal list --json | jq '.[] | .subject'
```

## Config

`~/.config/ms365-bridge/config.toml`:

```toml
email = "tommy.falkowski@iem.fraunhofer.de"
oama_cmd = "oama access {email}"
server = "outlook.office365.com"
```
