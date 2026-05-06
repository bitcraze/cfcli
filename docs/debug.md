# Debug

Debugging utilities that are useful when tracking down firmware issues.

## Dump assert info

If the Crazyflie has hit an assert, the firmware keeps a snapshot in memory. The
`debug assert` command triggers the firmware to print that snapshot on the
console (same mechanism as the "Assert info" button in the cfclient Console
tab):

```text
cfcli debug assert
```

The command sets the `system.assertInfo` parameter, which causes the firmware
to emit a one-line snapshot on the console (CRTP CONSOLE port). cfcli prints
that line and exits. The leading `SYS:` subsystem prefix that the firmware
adds is stripped so the output is just the assert text.

If the firmware reports no stored assert it prints `No assert information
found`. cfcli falls back to printing `No assert info` only when the firmware
doesn't respond at all (e.g. link drop).

### Tuning the wait timeout

`--wait-timeout-ms` (default `1500`) bounds how long cfcli waits for the
assert line. Bump it on slow/lossy radio links if the line isn't arriving
in time:

```text
cfcli debug assert --wait-timeout-ms 3000
```

For an outer wall-clock cap (e.g. when scripting), use the global
`--timeout` flag — `cfcli debug assert` is a bounded command, so a
`--timeout` hit exits **40**.
