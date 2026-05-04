# Memory access

The memory subsystem allows reading and writing raw binary data to various
memories and memory-mapped functionalities in the Crazyflie. In the Crazyflie
CLI the **mem** command is used to access the memory subsystem as raw binary
data. Some memories also support decoding of the data by using the **display**
parameter.

Note that all command accept both decimal and hexadecimal (prefix `0x`) values for
address, size and data parameters.

For documentation on the memory sub system and the various memory types see [this link](https://www.bitcraze.io/documentation/repository/crazyflie-firmware/master/functional-areas/memory-subsystem/)

## Selecting a memory

Most `mem` subcommands take a memory reference as the first positional argument.
Three forms are accepted:

* **Numeric ID** — the ID printed by `mem list`, e.g. `5` or `0x05`. IDs are an
  enumeration of the memories currently reported by the Crazyflie and may shift
  if memories are added or removed, so prefer the type-name form when writing
  guides or scripts.
* **Type name** — a `MemoryType` variant, e.g. `DeckCtrlDFU`. Resolves to that
  memory if exactly one of the type exists.
* **Type with instance index** — `Type:N`, e.g. `DeckCtrlDFU:0`. Required when
  multiple memories of the same type are present.

If a type name is given but multiple memories of that type are present and no
instance index was supplied, the command errors out and prints the valid range
of instance indices.

## Listing memories

To list all the available memories in the Crazyflie use the following command:

```bash
cfcli mem list
```

This will show an output similar to the one below showing id, type, size and serial (if available):

```text
Memories:
[0] EEPROMConfig size=7k (0x1fff/8191)
[1] Trajectory size=4k (0x1000/4096)
[2] MemoryTester size=4k (0x1000/4096)
[3] DeckCtrl size=2k (0x800/2048) (0x2D0043000550314854363720)
[4] DeckMemory size=1310720k (0x50000000/1342177280)
[5] DeckCtrlDFU size=96k (0x18000/98304)
```

## Reading memory

`mem read` defaults to offset `0` and length `32`, so a quick peek at a memory
is just:

```bash
cfcli mem read DeckCtrlDFU
```

To read a specific range, use `--offset` (`-s` for "seek") and `--length`
(`-n`). Both accept decimal or hex (`0x...`) values:

```bash
cfcli mem read DeckCtrlDFU --offset 0x00 --length 0x20
cfcli mem read DeckCtrlDFU -s 0x00 -n 0x20
```

This will read 0x20 (32) bytes from the `DeckCtrlDFU` memory starting at
address 0x00. The output will look similar to this:

```text
0000:   30 78 42 43 01 3c 02 00 00 00 00 00 00 00 00 e7   0xBC.<..........
0010:   e7 e7 e7 e7 ef ff ff ff ff ff ff ff ff ff ff ff   ................
```

Memories can also be addressed by numeric ID:

```bash
cfcli mem read 5 -s 0x00 -n 0x20
```

It's also possible to write the data to a file directly using `--output` (`-o`):

```bash
cfcli mem read DeckCtrlDFU -s 0x00 -n 0x20 -o memory_dump.bin
```

## Writing memory

To write raw binary data to the memory use the following command. This will
write the bytes 1, 1, 2 to the `EEPROMConfig` memory starting at address 0x20.

```bash
cfcli mem write EEPROMConfig -s 0x20 --data 0x01,1,0x02
```

It's also possible to write data from a file using `--input` (`-i`):

```bash
cfcli mem write EEPROMConfig -s 0x20 -i memory_data.bin
```

**Note**: The Crazyflie will most likely not like writing raw random data to memories, so
when using this functionality make sure you know what you are doing!

## Displaying memory

This command is used to interpret and display the content of a memory. Note that not all
memories support this functionality.

```bash
cfcli mem display EEPROMConfig
```

This will give an output similar to this:

```text
EEPROM Config:
  Radio Channel: 60
  Radio Speed: 2 Mbps
  Pitch Trim: 0.0000
  Roll Trim: 0.0000
  Radio Address: [E7, E7, E7, E7, E7]
```

If no memory is given, an interactive picker is shown.

## Example usage

Below is an example of how to use the raw memory access to bootload the Color
LED deck. Using the `DeckMemory` type name keeps the commands stable across
firmwares — the numeric ID may differ between Crazyflies (note this only works for one deck
since otherwise there's an offset in the memory address).

```bash
# Switch the deck into bootloader mode
cfcli mem write DeckMemory -s 0x1004 --data 0x02
# Write the firmware binary to the deck memory
cfcli mem write DeckMemory -s 0x10000000 -i color-led.bin
# Switch the deck back into application mode
cfcli mem write DeckMemory -s 0x1004 --data 0x01
```
