# Lighthouse Positioning System

The `lh` command manages the Lighthouse positioning system configuration stored
on the Crazyflie. It can read, write and display the base station geometry and
calibration data needed for lighthouse-based positioning.

```text
Usage: cfcli lh <COMMAND>

Commands:
  config  Base station geometry and calibration configuration
  help    Print this message or the help of the given subcommand(s)
```

## Background

A Crazyflie configured for Lighthouse positioning needs two pieces of data per
base station:

- **Geometry** — the base station's pose (origin + rotation matrix) in the
  flight space. Produced by running an estimation procedure (e.g. cfclient's
  geometry estimation or a manual measurement).
- **Calibration** — the base station's intrinsic sweep parameters
  (`phase`, `tilt`, `curve`, `gibmag`/`gibphase`, `ogeemag`/`ogeephase` for
  each of two sweeps, plus the base station UID). This data is broadcast by
  the base stations themselves and stored on the Crazyflie.

Both are kept in a dedicated lighthouse memory on the Crazyflie. There are 16
base station slots (IDs `0..15`), each marked valid or invalid.

## Config

```text
Usage: cfcli lh config <COMMAND>

Commands:
  display  Display lighthouse configuration in human-readable form
  read     Read lighthouse configuration as YAML (to file or stdout)
  write    Write lighthouse configuration from YAML (from file or stdin)
```

### YAML File Format

The format matches the one used by the Python `cflib` so configurations can be
shared with cfclient.

```yaml
type: lighthouse_system_configuration
version: '2'
systemType: 2
geos:
  0:
    origin: [-0.5228, -0.8784, 2.2364]
    rotation:
      - [ 0.3832, -0.8521,  0.3565]
      - [ 0.5376,  0.5196,  0.6640]
      - [-0.7511, -0.0627,  0.6572]
  1:
    origin: [1.8885, -0.9296, 2.4064]
    rotation:
      - [-0.3296, -0.4936, -0.8048]
      - [ 0.4440, -0.8333,  0.3293]
      - [-0.8332, -0.2488,  0.4939]
calibs:
  0:
    uid: 2360210604
    sweeps:
      - phase: 0.0
        tilt: -0.051
        curve: 0.275
        gibmag: -0.005
        gibphase: 2.281
        ogeemag: -0.184
        ogeephase: 1.847
      - phase: -0.005
        tilt: 0.051
        curve: 0.211
        gibmag: -0.004
        gibphase: 2.219
        ogeemag: 0.073
        ogeephase: 2.213
```

Top-level fields:

- `type` — file type marker, always `lighthouse_system_configuration`
- `version` — file format version, currently `'2'`
- `systemType` — `1` for V1 base stations, `2` for V2
- `geos` — map of `bs_id -> { origin, rotation }`
- `calibs` — map of `bs_id -> { uid, sweeps[2] }`

Either map can be omitted or empty if you only want to read/write one half.

### Display

Render the current configuration in human-readable form, either from the
Crazyflie or from a YAML file.

```text
cfcli lh config display [-i <FILE>]
```

Options:

- `-i, --input <FILE>` — read from a YAML file instead of the Crazyflie

When `--csv` is used (the global flag), `display` emits a long-format CSV
with the schema `section,bs_id,key,value`, e.g.:

```text
section,bs_id,key,value
geo,0,origin_x,-0.5228
geo,0,origin_y,-0.8784
geo,0,origin_z,2.2364
geo,0,rotation_0_0,0.3832
...
cal,0,uid,2360210604
cal,0,sweep0_phase,0.0
cal,0,sweep0_tilt,-0.051
...
```

The same schema covers both geometry (`section=geo`) and calibration
(`section=cal`) so consumers can filter with `awk -F,` or `grep`.

#### Display Examples

```text
# Pretty print what's currently on the Crazyflie
cfcli lh config display

# Pretty print a YAML file (no Crazyflie connection)
cfcli lh config display -i my_setup.yaml

# Machine-readable CSV
cfcli lh config display --csv

```

### Read

Read the configuration from the Crazyflie and emit YAML.

```text
cfcli lh config read [-o <FILE>]
```

Options:

- `-o, --output <FILE>` — write YAML to a file. If omitted, the YAML is written
  to stdout and informational messages go to stderr so the output can be piped
  directly.

Read iterates all 16 base station slots and includes only those marked valid.

#### Read Examples

```text
# Save the current config to a file
cfcli lh config read -o my_setup.yaml

# Pipe the YAML somewhere else
cfcli lh config read | tee backup.yaml

# Diff against a known-good file
diff <(cfcli lh config read) my_reference.yaml
```

### Write

Write a configuration from a YAML file (or stdin) to the Crazyflie.

```text
cfcli lh config write [-i <FILE>]
```

Options:

- `-i, --input <FILE>` — read YAML from a file. If omitted, YAML is read from
  stdin.

Only the slots present in the YAML are written; existing slots not mentioned
in the YAML are left untouched. To force a slot to be cleared, omit it from
the YAML and re-flash, or use a separate erase tool.

#### Write Examples

```text
# Write a config from a file
cfcli lh config write -i my_setup.yaml

# Pipe YAML in from stdin
cat my_setup.yaml | cfcli lh config write
```

## Copy a Configuration Between Crazyflies

Read from one Crazyflie and write to another by piping `read` into `write`,
overriding the `--uri` for each:

```bash
cfcli --uri radio://0/80/2M/E7E7E7E7E7 lh config read \
  | cfcli --uri radio://0/80/2M/E7E7E7E7E8 lh config write
```
