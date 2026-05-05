# Settings

The `settings` command manages local CLI settings that are persisted between sessions.
These settings affect how cfcli behaves locally (e.g. scan addresses and connection timeout),
as opposed to `config` which configures the Crazyflie hardware itself.

```text
Usage: cfcli settings <COMMAND>

Commands:
  show     Show all current settings
  timeout  Manage the connection timeout
  address  Manage scan addresses
  help     Print this message or the help of the given subcommand(s)
```

## Show all settings

Display all current settings at once:

```bash
cfcli settings show
```

This prints the connection timeout, the configured scan addresses, and the path to the preserved console history file (used by the global `-p`/`--preserve-console` flag — see [Console](console.md)).

## Connection timeout

The connection timeout controls how long cfcli waits when connecting to a Crazyflie.
The default is 1000ms.

### Show the current timeout

```bash
cfcli settings timeout show
```

### Set the timeout

Set the timeout in milliseconds:

```bash
cfcli settings timeout set 2000
```

The timeout is appended as a query parameter to the URI at connection time (e.g.
`radio://0/80/2M/E7E7E7E7E7?timeout=2000`) but is not stored in the selected URI.

### Reset to default

```bash
cfcli settings timeout clear
```

## Scan addresses

Scan addresses control which radio addresses are scanned when using the `scan` and
`select` commands. The default is a single address: `E7E7E7E7E7`. When multiple
addresses are configured, all of them are scanned and results are deduplicated.

An address provided as an argument to `scan` or `select` overrides the configured
addresses for that invocation.

### List configured addresses

```bash
cfcli settings address list
```

### Add an address

```bash
cfcli settings address add E7E7E7E701
```

Addresses are 5-byte hex strings (10 hex characters). Duplicate addresses are
not added.

### Remove an address

```bash
cfcli settings address remove E7E7E7E701
```

If all addresses are removed, the list is automatically reset to the default
(`E7E7E7E7E7`).

### Reset to default

```bash
cfcli settings address clear
```
