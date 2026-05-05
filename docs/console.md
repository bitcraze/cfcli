# Crazyflie console

This module provides access to the Crazyflie console.

**NOTE:** The console data history clears when you connect to the Crazyflie (i.e is downloaded on connect)

## Show console prints

This command shows everything printed in the Crazyflie console.

```text
cfcli console
```

If you do not want any formatting of the text then use the ```--no-format``` parameter:

```text
cfcli console --no-format
```

## Preserve console across connections

Normally, console data is only available while connected. With the ```--preserve-console``` (```-p```) global flag, console output is saved to a file during every connection. When running multiple commands in a row the console data is accumulated:

```text
cfcli -p param set motorPowerSet.enable 1
cfcli -p log print stabilizer.roll --period 100
```

When the ```console``` command is executed, any saved console history is always printed first and then cleared, followed by the live console output:

```text
cfcli console
```

This is useful for capturing console debug output that was printed during other operations (e.g. parameter changes or log sessions).

## Clear preserved console history

The `--clear` flag deletes the preserved console history file and exits without connecting to a Crazyflie. Useful when you want to discard accumulated output between runs:

```text
cfcli console --clear
```

The file path is shown by `cfcli settings show`.

## Stop streaming after a fixed duration

`console` is a streaming command — by default it runs until the link is broken. Combine it with the global `--timeout` flag to stop cleanly after a fixed wall-clock duration:

```text
cfcli --timeout 3000 console
```

When `--timeout` fires on a streaming command, the process exits **0** (the timer is the intended way to stop it). This is the recommended pattern when running `cfcli console` from a script or CI step.
