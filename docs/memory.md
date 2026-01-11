# Memory access

The memory subsystem allows reading and writing raw binary data to various
memories and memory-mapped functionalities in the Crazyflie. In the Crazyflie
CLI the **mem** command is used to access the memory subsystem as raw binary
data. Some memories also support decoding of the data by using the **display**
parameter.

Note that all command accept both decimal and hexadecimal (prefix `0x`) values for
address, size and data parameters.

For documentation on the memory sub system and the various memory types see [this link](https://www.bitcraze.io/documentation/repository/crazyflie-firmware/master/functional-areas/memory-subsystem/)

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

To read raw binary data from a memory use the following commands:

```bash
cfcli mem read 0 0x00 0x20
```

This will read 0x20 (32) bytes from memory ID 0 starting at address 0x00. The output
will look simlar to this:

```text
0000:   30 78 42 43 01 3c 02 00 00 00 00 00 00 00 00 e7   0xBC.<..........
0010:   e7 e7 e7 e7 ef ff ff ff ff ff ff ff ff ff ff ff   ................
```

It's also possible to write the data to a file direclyly using the `--output` parameter:

```bash
cfcli mem read 0 0x00 0x20 --output memory_dump.bin
```

## Writing memory

To write raw binary data to the memory use the following command. This will write the data
1,2,3 to the memory with ID 0 start at the address 0x20.

```bash
cfcli mem write 0 0x20 --data 0x01,1,0x02
```

It's also possible to write data from a file using the `--input` parameter:

```bash
cfcli mem write 0 0x20 --input memory_data.bin
```

**Note**: The Crazyflie will most likely not like writing raw random data to memories, so
when using this functionality make sure you know what you are doing!

## Displaying memory

This command is used to interpret and display the content of a memory. Note that not all
memories support this functionality.

```bash
cfcli mem display 6
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

## Example usage

Below is an example of how to use the raw memory access to bootload the Color
LED deck (the *DeckMemory* is on ID 3).

```bash
# Switch the deck into bootloader mode
cfcli mem write 3 0x1004 --data 0x02
# Write the firmware binary to the deck memory
cfcli mem write 3 0x10000000 --input color-led.bin
# Switch the deck back into application mode
cfcli mem write 3 0x1004 --data 0x01
```
