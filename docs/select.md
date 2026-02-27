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
