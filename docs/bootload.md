# Crazyflie bootloader

This module provides access to the Crazyflie bootloader as well as the deck flashing
subsystem. It supports the following commands:

```text
Usage: cfcli bootload <COMMAND>

Commands:
  info      Print bootloader information
  releases  List available releases
  targets   List of hardcoded targets
  flash     Flash firmware to the device
  help      Print this message or the help of the given subcommand(s)
```

## Show bootloader information

This command shows you the bootloader version and other related information of the connected Crazyflie.

```text
cfcli bootload info
```

## List available releases

This command will list the available releases from the [Crazyflie release repository](https://github.com/bitcraze/crazyflie-release).

```text
cfcli bootload releases
```

## Flash firmware

Using the flash command you can flash releases, local ZIP files and BIN files. It's also possible
to combine the commands, where BIN files will take precidence over files in the release or ZIP files.

```text
Flash firmware to the device

Usage: cfcli bootload flash [OPTIONS] <--release [<RELEASE>]|--zip <ZIP>|--bin <BIN>>

Options:
      --release [<RELEASE>]
          Release name, interactive selection if left blank (cannot be combined with zip)

      --zip <ZIP>
          Release ZIP file path (cannot be combined with release)

      --bin <BIN>
          Comma-separated list of key=value pairs for targets and binary files.
          Note that these will override any files in release or zip.
          
          Example: stm32-fw=cf2_stm.bin,nrf51-fw=cf2_nrf.bin

      --targets [<TARGETS>]
          Comma-separated list of targets to flash, interactive selection if
          left blank. By default all targets found in the release/zip/bin will
          be flashed.
          
          Example: stm32-fw,nrf51-fw

      --cold
          Use coldboot (i.e rescue mode) to flash the device
```

### Flash examples

#### Releases

The following command will let you flash a release (which you select) and only flash the specified targets (which you select):

```text
cfcli bootload flash --release --targets
```

The following command will flash the 2025.12 release to the nRF51 (FW) and to the top Color LED deck:

```text
cfcli bootload flash --release 2025.12 --targets nrf51-fw,bcColorLedTop:col-fw
```

#### ZIP files

The following command will let you flash the firmware in a ZIP package and only flash the specified targets (which you select):

```text
cfcli bootload flash --zip my-fw.zip --targets
```

You can also specify exactly which targets to flash from the ZIP:

```text
cfcli bootload flash ---zip my-fw.zip --targets nrf51-fw,bcColorLedTop:col-fw
```

#### BIN files (and overloading)

When flashing BIN files any files you specify will overload the files
in the release or ZIP file. Since the bin files do not contain where
they should be flashed you can either set the target on the commandline
or leave it blank to select.

The following command will promt you to select what target to flash the firmware
bin to:

```text
cfcli bootload flash --bin firmware.bin
```

It's also possible to set the target directly and also to flash multiple bins. The command
below will promt you for where you want to flash ```color-led-firmware.bin``` and will
then flash it to the selected target as well as flashing the ```nrf51-firmware.bin```
to the nRF51.

```text
cfcli bootload flash --bin nrf51-fw=nrf51-firmware.bin,color-led-firmware.bin
```

The bin parameter can be combined with either ```release``` or ```zip``` to flash firmware from these sources but overloaded with the binaries supplied. This command
will download and flash the ```2025.12``` release, but replace the nRF51 binary with
the ```custom-nrf51-firmware.bin``` file:

```text
cfcli bootload flash --release 2025.12 --bin nrf51-fw=custom-nrf51-firmware.bin
```

#### Recovery mode boot

In order to access the recovery mode booting use the ```--cold``` option. If
this is enabled the URI set using ```select``` will be disregarded and the
default will be used.

```text
cfcli bootload flash --release 2025.12 --cold
```
