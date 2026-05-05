# Select Crazyflie

The `select` command scans for available Crazyflies and saves the selected URI
for use in subsequent commands.

## Interactive selection

To scan and interactively select a Crazyflie:

```text
cfcli select
```

If you have a Crazyflie on a different address than the default you can specify
it while scanning:

```text
cfcli select E7E7E7E7E7
```

## Automatic selection

Use the `--auto` flag to automatically select the URI when exactly one Crazyflie
is found. This is useful for scripting and automation.

```text
cfcli select --auto
```

If zero or more than one Crazyflie is found, the command will exit with an error.

## Select from USB

Use the `--from-usb` flag to connect to a USB-attached Crazyflie, read its radio
configuration (channel, speed, and address) from the EEPROM, and save the
corresponding radio URI.

```text
cfcli select --from-usb
```

This is useful when you have a Crazyflie connected via USB and want to configure
cfcli to connect to it over radio in subsequent commands. The command will read
the radio channel, speed, and address from the Crazyflie's EEPROM and construct
a `radio://` URI.

If zero or more than one USB Crazyflie is found, the command will exit with an
error.

## Scan only

The `scan` command lists Crazyflies found on the configured scan addresses
without saving any selection:

```bash
cfcli scan
```

The default output is one URI per line, each prefixed with `>`. For
machine-readable output, add the global `--csv` flag:

```bash
cfcli --csv scan
```

```text
uri
usb://51004F000551343036333233
radio://0/80/2M/E7E7E7E7E7
```

The first line is a header (`uri`); each subsequent line is one discovered URI.
An empty result (just the header line) means no Crazyflies were found.
