# Access platform functionality

This module provides access to platform-specific functionality of the Crazyflie, like various
system information and power management features.

## Show platform information

This command shows you firmware/CRTP protocol version and platform type of the connected Crazyflie.

```bash
cfcli platform info
```

This will show something like this:

```text
Platform        : Crazyflie 2.1
Firmware        : 2025.09.1 +100
CRTP protocol   : 10
```

For machine-readable output, add the global `--csv` flag. Each row is a
`field,value` pair:

```bash
cfcli --csv platform info
```

```text
field,value
platform,Crazyflie 2.1
firmware,2025.09.1 +100
crtp_protocol,10
```

## Power management

The following commands are available for power management of the Crazyflie:

This will reboot the Crazyflie (by power-cycling the STM32 power domain):

```bash
cfcli platform reboot
```

This will put the Crazyflie to sleep and wake it up again (powering off the STM32 and decks):

```bash
cfcli platform sleep
cfcli platform wakeup
```

This will power off the Crazyflie (same as pressing the power button):

```bash
cfcli platform power-off
```

## Restart the STM32 into USB DFU

This restarts the STM32 into its ROM bootloader, the same as holding the power button from off,
so it can be flashed over USB with a DFU tool such as `dfu-util`:

```bash
cfcli platform dfu
```

The command is handled by the nRF51, so the Crazyflie has to be selected with a radio URI, and its
nRF51 firmware has to support it. With the Crazyflie connected over USB, `cfcli` waits for the STM32
to show up in DFU mode (`0483:df11`):

```text
STM32 is in USB DFU mode (0483:df11)
When done, restart it into its firmware with: cfcli platform reboot
```

Once flashing is done, `cfcli platform reboot` restarts the STM32 into its firmware.
