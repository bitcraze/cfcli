# Crazyradio

The **cr** command provides direct access to the Crazyradio dongle for
operations that do not require a connection to a Crazyflie. This includes
listing connected dongles, sniffing radio traffic and broadcasting packets.

## Listing Crazyradios

To list all connected Crazyradio dongles:

```bash
cfcli cr list
```

This will show information about each dongle including serial number, firmware
version and USB bus location.

## Sniffing packets

To sniff broadcast packets on a given channel and address:

```bash
cfcli cr sniff
```

By default this listens on channel 80, datarate 2M and address E7E7E7E7E7.
These can be changed with the `--channel`, `--datarate` and `--address` options:

```bash
cfcli cr sniff --channel 78 --datarate 2 --address FFE7E7E7E7
```

Received packets are printed with pipe, RSSI, timestamp and payload data.

## Broadcasting a packet

To broadcast a packet (without acknowledgement) on a given channel and address
use the **broadcast** subcommand. Data can be provided either inline or from a
file.

### Inline data

Provide bytes as a comma-separated list using `--data`. Both decimal and
hexadecimal (prefix `0x`) values are supported:

```bash
cfcli cr broadcast --data 0x8F,0x07,0x00,0x00,0x00,0x00,0x3F,0x00
```

### Data from a file

Provide raw binary data from a file using `--input` (or `-i`):

```bash
cfcli cr broadcast --input packet.bin
```

### Options

All broadcast options with their defaults:

| Option | Description | Default |
|---|---|---|
| `--radio`, `-r` | Crazyradio index (0-based) | 0 |
| `--channel`, `-c` | Radio channel (0-125) | 80 |
| `--datarate`, `-d` | Datarate: 0=250K, 1=1M, 2=2M | 2 |
| `--address`, `-a` | 5-byte broadcast address (hex) | FFE7E7E7E7 |
| `--data` | Comma-separated bytes to send | |
| `--input`, `-i` | File to read raw binary data from | |

Either `--data` or `--input` must be provided.
