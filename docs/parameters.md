# Parameters

The parameter subsystem provides access to configuration variables on the Crazyflie. Parameters
can be read and written, and some support persistent storage in EEPROM so they survive reboots.

For more information on how to use the logging and parameter subsystems see [this link](https://www.bitcraze.io/documentation/repository/crazyflie-firmware/master/userguides/logparam/)

```text
Usage: cfcli param <COMMAND>

Commands:
  list        List all available variables
  get         Read the value of a parameter
  set         Set the value of a parameter
  store       Store the current value of a parameter to EEPROM
  clear       Clear a stored parameter value from EEPROM (reverts to firmware default)
  help        Print this message or the help of the given subcommand(s)
```

## List available parameters

```bash
cfcli param list
```

This will produce an output similar to this:

```text
Name                           | Access | Persistent | Value/Stored
-------------------------------|--------|------------|------------
activeMarker.back              |   RW   | Yes        | U8(1)
activeMarker.front             |   RW   | Stored     | U8(3)/U8(3)
commander.enHighLevel          |   RW   |            | U8(0)
firmware.revision0             |   RO   |            | U16(14906)
...
```

Where `Access` is either `RW` (read-write) or `RO` (read-only), and `Persistent`
shows whether the parameter supports EEPROM storage (`Yes`), has a value currently
stored (`Stored`), or is blank if not persistent. The `Value/Stored` column shows
the current value, and when a value is stored, shows `value/stored`.

### CSV output

Adding the global `--csv` flag emits machine-readable rows with a stable
schema. `param list` and `param get` share the same columns, so consumers can
parse either with one parser:

```bash
cfcli --csv param list
```

```text
name,access,persistent,default,stored_value,value
activeMarker.back,RW,yes,1,3,3
activeMarker.front,RW,yes,1,,1
commander.enHighLevel,RW,no,,,0
firmware.revision0,RO,no,,,14906
```

Columns:

* `name` — `group.name`
* `access` — `RW` or `RO`
* `persistent` — `yes` (parameter supports EEPROM storage), `no`, or `error`
* `default` — firmware default value (empty for non-persistent parameters)
* `stored_value` — value currently stored in EEPROM (empty if not stored)
* `value` — current value

Values are plain numbers (e.g. `42`, `4.222874`) — not the `U8(42)` wrapper
form used in the human-readable view — so they can be piped into other tools
without further parsing.

## Get parameter values

Read the value of one or more parameters by specifying comma-separated names:

```bash
cfcli param get ring.effect,activeMarker.back
```

The output includes persistent storage information for each parameter:

```text
Name                           | Access | Persistent | Default         | Stored Value    | Value
-------------------------------|--------|------------|-----------------|-----------------|------
ring.effect                    |   RW   | No         | U8(0)           |                 | U8(6)
activeMarker.back              |   RW   | Yes        | U8(1)           | U8(3)           | U8(3)
```

The `Persistent` column shows whether a value is stored in EEPROM (`Yes`/`No`),
`Default` shows the firmware default, and `Stored Value` shows the EEPROM value if any.
Parameters that don't support persistence have these columns blank.

`param get` also supports `--csv` and uses the same column layout as `param list`
(see [CSV output](#csv-output) above). Unknown parameter names produce a clear
error and exit code 20.

If no names are provided, an interactive selection prompt will be shown.

## Set parameter values

Set one or more parameters using comma-separated `name=value` pairs:

```bash
cfcli param set ring.effect=10,ring.solidRed=100
```

If no parameters are provided, an interactive prompt will let you select writable
parameters and enter values.

### Persist after setting

Use `--store` to automatically store the value to EEPROM after setting it, so
it survives reboots:

```bash
cfcli param set ring.effect=10 --store
```

## Persistent parameters

Some parameters support persistent storage in the Crazyflie's EEPROM. When stored,
these values are automatically restored on boot instead of using the firmware default.
The `get` command shows the persistent state for each parameter.

### Store to EEPROM

Store the current value of a parameter to EEPROM so it persists across reboots:

```bash
cfcli param store ring.effect
```

### Clear from EEPROM

Clear a stored value from EEPROM, reverting the parameter to its firmware default
on the next boot:

```bash
cfcli param clear ring.effect
```
