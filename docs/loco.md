# Loco Positioning System

The `loco` command works with the Loco Positioning System (LPS): it displays
live anchor information from the Loco Positioning v2 memory on the Crazyflie,
and reads/writes anchor position configuration files. This requires an LPS deck
to be attached.

```text
Usage: cfcli loco <COMMAND>

Commands:
  display  Display Loco Positioning System anchor information
  config   Anchor position configuration
  help     Print this message or the help of the given subcommand(s)
```

## Display anchor data

To display the configured anchors, their positions, and active status:

```bash
cfcli loco display
```

This will show output similar to:

```text
Loco Positioning System - Anchor Data:
   ID  Active  Valid  Position (x, y, z)
    0     yes    yes  (1.000, 2.000, 0.500)
    1     yes    yes  (4.000, 2.000, 0.500)
    2     yes    yes  (1.000, 5.000, 0.500)
    3     yes    yes  (4.000, 5.000, 0.500)
    4      no    yes  (0.000, 0.000, 3.000)
    5      no    yes  (4.000, 0.000, 3.000)
```

The columns show:

- **ID** - The anchor identifier
- **Active** - Whether the anchor is currently being used for positioning
- **Valid** - Whether the anchor has valid position data stored
- **Position** - The 3D coordinates (x, y, z) of the anchor in meters

Add `--csv` for a machine-readable version of the same table.

## Config

```text
Usage: cfcli loco config <COMMAND>

Commands:
  display  Display anchor positions in human-readable form
  read     Read anchor positions as YAML (to file or stdout)
  write    Write anchor positions from YAML (from file or stdin) to the anchors
```

### Background

Unlike the Lighthouse system, anchor positions are stored *in the anchors*, not
on the Crazyflie. Each anchor broadcasts its own position over UWB and the
Crazyflie collects what it hears into the Loco Positioning v2 memory.

That means:

- `read` reports the positions the Crazyflie has picked up from the anchors —
  the equivalent of cfclient's "Configure positions" → "Get from anchors" →
  "Save to file…".
- `write` pushes positions *to the anchors* using LPP short packets relayed by
  the Crazyflie — the equivalent of cfclient's "Write to anchors". These
  packets are best effort, so `write` reads the positions back and resends to
  any anchor that has not picked up its new position yet.

### YAML File Format

The format matches the one cfclient writes, so files can be shared between the
two tools (and with [swarmkeeper](https://github.com/bitcraze)'s "Load anchor
positions…"). It is a plain map of anchor id to position in meters:

```yaml
0:
  x: 1.0
  y: 2.0
  z: 0.5
1:
  x: 4.0
  y: 2.0
  z: 0.5
5:
  x: -0.5
  y: 5.25
  z: 3.0
```

### Display

Display anchor positions, either from a file or from the Crazyflie:

```text
cfcli loco config display [-i <FILE>]
```

| Option | Description |
| --- | --- |
| `-i`, `--input <FILE>` | YAML file to display. Without it, reads from the Crazyflie (no connection is made when a file is given). |

```bash
# From the connected Crazyflie
cfcli loco config display

# From a file, no Crazyflie needed
cfcli loco config display -i my_anchors.yaml
```

### Read

Read the anchor positions from the Crazyflie and emit them as YAML:

```text
cfcli loco config read [-o <FILE>] [--include-invalid]
```

| Option | Description |
| --- | --- |
| `-o`, `--output <FILE>` | File to write the YAML to. Without it, the YAML goes to stdout. |
| `--include-invalid` | Also include anchors whose position is not marked valid (normally these are skipped and listed on stderr). |

```bash
# Save to a file
cfcli loco config read -o my_anchors.yaml

# Or to stdout
cfcli loco config read | tee backup.yaml
```

Anchors only show up once the Crazyflie has heard from them, so make sure the
anchors are powered and the system is running (check with `cfcli loco display`)
before reading.

### Write

Write anchor positions from YAML to the anchors:

```text
cfcli loco config write [-i <FILE>] [--no-verify] [--verify-timeout <SECONDS>]
```

| Option | Description |
| --- | --- |
| `-i`, `--input <FILE>` | YAML file to read the positions from. Without it, the YAML is read from stdin. |
| `--no-verify` | Send the positions once and exit without reading them back. |
| `--verify-timeout <SECONDS>` | How long to keep resending until every anchor confirms its new position (default: 15). |

```bash
# From a file
cfcli loco config write -i my_anchors.yaml

# Pipe YAML in from stdin
cat my_anchors.yaml | cfcli loco config write
```

The command resends to any anchor that has not confirmed and prints the
progress:

```text
Confirmed 4/6 anchors
Confirmed 6/6 anchors
All 6 anchor positions written and confirmed
```

If some anchors never confirm within the timeout the command lists them and
exits with code 40 (timeout) — usually a sign that the anchor is powered off or
out of range. Anchors with an id of 16 or above are sent but cannot be verified,
because the Loco Positioning v2 memory only exposes ids `0..15`; they are
reported on stderr.

## Copy a Configuration From One System to Another

Read from a Crazyflie flying in one system and write the positions into the
anchors of another by piping `read` into `write`:

```bash
cfcli --uri radio://0/80/2M/E7E7E7E7E7 loco config read \
  | cfcli --uri radio://0/80/2M/E7E7E7E7E8 loco config write
```
