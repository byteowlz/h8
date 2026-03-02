# Issues

## Open

### [trx-ybfm.2] Natural language resource queries (P1, feature)
Support natural language queries for resource availability that feel conversational rather than requiring rigid subcommand syntax.

## Examples

```bash
...


### [trx-ybfm] Resource groups: query availability of named resource collections (P1, feature)
Add a [resources] config section for grouping related EWS resource mailboxes (cars, rooms, equipment) under a single name, and a new "h8 resource" command to query them.

## Config

```toml
...


### [trx-bf2h] ppl free/agenda silently returns own data for unresolvable emails (P2, bug)
When querying free/busy or agenda for an email address that doesn't resolve to a real mailbox in EWS, the command silently returns data (likely the user's own calendar) instead of reporting an error. This makes it impossible to tell if an email address is valid or not. The EWS GetUserAvailability endpoint returns an empty calendar_events list for unresolvable addresses, which gets interpreted as 'fully free'.

Suggested fix: Use exchangelib's ResolveNames service to validate the email before querying free/busy. If it doesn't resolve, return an error. Note: ResolveNames requires setting _version_hint manually (svc._version_hint = account.version) due to an exchangelib quirk.

### [trx-ybfm.1] Resource group description field for richer output (P3, feature)
Allow optional description metadata per resource in a group so output is more readable.

Option A - inline table:
```toml
[resources.cars]
...


### [trx-ddt9] Add resolve-names / GAL search command (P3, feature)
Add an 'h8 addr resolve <query>' or 'h8 contacts resolve <query>' command that uses EWS ResolveNames to search the Global Address List (GAL). This is useful for finding resource mailboxes (rooms, cars, equipment) whose email addresses are not in the user's contacts or cached addresses.

Example usage:
  h8 addr resolve 'IEM.M'     -> finds resource.m-em-1725e@iem.fraunhofer.de etc.
  h8 addr resolve 'resource'  -> finds all resource mailboxes
...


