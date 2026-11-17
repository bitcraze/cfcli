# Crazyflie console

Use `cfcli console` to read the output of one of the Crazyflie's consoles. Each
console has a path, such as `cf:stm32` for the Crazyflie's STM32 or `deck:bcCam`
for the Camera Deck. `cf:stm32` is available on every firmware version; other
consoles are advertised by firmware using CRTP protocol 13 or newer.

## Choose a console

Running `cfcli console` in a terminal asks which console to show:

```text
Select a console:
> cf:stm32
  deck:bcCam
```

The question is skipped when the Crazyflie only has one console.

To choose on the command line, give the exact, case-sensitive path with
`--source`. This is required in non-interactive use, including whenever stdin is
not a terminal; without it cfcli exits with code 30 before connecting.

```text
cfcli console --source cf:stm32
cfcli console --source deck:bcCam
```

If the console does not exist, cfcli exits with resource-not-found code 20 and
reports the available paths.

Only one console can be shown at a time; concurrent output from several consoles
may be added later.

## List the consoles

```text
cfcli console --list
```

Use global `--csv` to emit a `path` header and one CSV row per console.

## What is shown

The command replays the output already received for the console and then
continues with live output. Replay may include output from a previous connection
if the library received it before discovering the consoles. The library keeps up
to 64 KiB of such early packets and drops the oldest packets when full. Packets
lost over radio cannot be recovered.

If you do not want any formatting of the text then use the ```--no-format``` parameter:

```text
cfcli console --source cf:stm32 --no-format
```

## Preserve console across connections

Normally, console data is only available while connected. With the ```--preserve-console``` (```-p```) global flag, console output is saved to a file during every connection. When running multiple commands in a row the console data is accumulated:

```text
cfcli -p param set motorPowerSet.enable 1
cfcli -p log print stabilizer.roll --period 100
```

When the ```console``` command shows `cf:stm32`, any saved console history is printed first and then cleared, followed by the live console output:

```text
cfcli console --source cf:stm32
```

This is useful for capturing console debug output that was printed during other operations (e.g. parameter changes or log sessions).

Preservation currently applies only to `cf:stm32`. Showing another console
neither prints nor clears the preserved file.

## Clear preserved console history

The `--clear` flag deletes the preserved console history file and exits without connecting to a Crazyflie. Useful when you want to discard accumulated output between runs:

```text
cfcli console --clear
```

The file path is shown by `cfcli settings show`.

## Stop streaming after a fixed duration

Console output is a streaming command: by default it runs until the link is
broken. Combine it with the global `--timeout` flag to stop cleanly after a fixed
wall-clock duration:

```text
cfcli --timeout 3000 console --source deck:bcCam
```

When `--timeout` fires on a streaming command, the process exits **0** (the
timer is the intended way to stop it). cfcli then makes a clean attempt to close
the console, bounded to one additional second, before disconnecting. This is the
recommended pattern when running `cfcli console` from a script or CI step.

The timeout covers the whole command, including the time spent answering the
question, so give `--source` when combining the two.

Ctrl-C also stops the console with the same bounded close attempt before
disconnecting, then exits with code 130. On Unix, SIGTERM follows the same
cleanup path and exits with code 143.

`--list` is bounded rather than streaming. If its global timeout
expires, cfcli exits with timeout code 40.
