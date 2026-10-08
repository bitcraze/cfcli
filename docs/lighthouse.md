# Lighthouse Positioning System

The `lh` command manages the Lighthouse positioning system configuration stored
on the Crazyflie. It can read, write, display and check the base station
geometry and calibration data needed for lighthouse-based positioning. Only
Lighthouse V2 base stations are supported.

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
  the base stations themselves and stored on the Crazyflie. When a base station
  with another UID shows up on a channel, the Crazyflie takes that base
  station's calibration and stores it.

Both are kept in a dedicated lighthouse memory on the Crazyflie, with a slot
per base station ID, each marked valid or invalid. The memory has room for 16
(IDs `0..15`), but the firmware supports as many as it was built for:
4 by default (IDs `0..3`), up to 16 with a larger
`CONFIG_DECK_LIGHTHOUSE_MAX_N_BS`. cfcli finds the number from the size of
the lighthouse memory.

## Config

```text
Usage: cfcli lh config <COMMAND>

Commands:
  list     List the stored lighthouse configs: local ones and shared ones (<org>/<config>)
  display  Display a lighthouse configuration: a stored one, a file, or the Crazyflie's
  read     Read the Crazyflie's lighthouse configuration as YAML (to file or stdout)
  save     Store the Crazyflie's lighthouse configuration as <CONFIG> (new, or an update)
  write    Write a lighthouse configuration to the Crazyflie
  check    Compare the Crazyflie's lighthouse configuration with one
  import   Store a lighthouse configuration file (from the Crazyflie client or read)
  export   Write a stored lighthouse configuration to a file the Crazyflie client opens (or stdout)
  delete   Delete a stored lighthouse configuration
  move     Share a lighthouse config (cage -> org/cage), take it back (org/cage -> cage), or rename it
  pull     Get the latest version of the shared lighthouse configs (needed with sync off)
  push     Upload changes to shared lighthouse configs made with sync off or without the server
```

`display`, `write` and `check` take a stored configuration by its ID, or a
file with `-i`. `write` and `check` also read YAML piped in. With none of
these, `write` lists the stored configurations to pick one from, and `check`
uses the configuration the selected swarm names (see
[Swarms](/docs/swarm.md#lighthouse)). Without a terminal, as for a command in
a script or a cron job, `write` uses the selected swarm's too.

### YAML File Format

The format matches the one used by the Python `cflib` so configurations can be
shared with cfclient.

```yaml
type: lighthouse_system_configuration
version: '1'
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
- `version` — file format version: cfcli writes `'1'`, the version cflib
  reads, and also reads `'2'`, which earlier cfcli versions wrote for the same
  format
- `systemType` — `2`, Lighthouse V2 base stations. A file for V1 base
  stations (`1`) is refused; when the field is missing, V2 is assumed, as in
  cflib
- `geos` — map of `bs_id -> { origin, rotation }`
- `calibs` — map of `bs_id -> { uid, sweeps[2] }`
- `name` — optional, the name shown for a stored configuration (cflib
  ignores it)

Either map can be omitted or empty if you only want to read/write one half.
The maps are written in base station order, so reading the same configuration
twice gives the same file. Other top-level fields are kept.

### Display

Render the current configuration in human-readable form, either from the
Crazyflie or from a YAML file.

```text
cfcli lh config display [<CONFIG> | -i <FILE>]
```

Options:

- `<CONFIG>` — a stored configuration instead of the Crazyflie's
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

# Pretty print a stored configuration
cfcli lh config display lab/cage

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

Read goes through the base station slots the firmware supports and includes
only those marked valid.

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

Write a configuration to the Crazyflie.

```text
cfcli lh config write [<CONFIG> | -i <FILE>]
```

Options:

- `<CONFIG>` — a stored configuration
- `-i, --input <FILE>` — read YAML from a file

With neither, YAML piped in is written. Otherwise, in a terminal, `write`
lists all the stored configurations (local ones and the shared ones in all
your organizations) to pick one from, starting at the one the selected swarm
names. Without a terminal it writes the selected swarm's configuration. Stdin
that isn't a terminal but has nothing in it (a script, a cron job) counts as
nothing piped in.

```text
$ cfcli lh config write
? Lighthouse config to write:
  cage - Local cage (4 base stations)
> lab/cage - The cage (4 base stations)
  other/room - Other room (4 base stations)
```

All base station slots the firmware supports are written. Slots present in
the YAML are uploaded as valid, while slots omitted from the YAML are written
as invalid to clear stale configuration. The resulting configuration is then
persisted to flash.

The file is checked before cfcli connects, and a configuration with base
station IDs the firmware doesn't support is refused before anything is
written (exit code 30), for example 8 base stations for a Crazyflie with the
default firmware.

#### Write Examples

```text
# Write a config from a file
cfcli lh config write -i my_setup.yaml

# Write a stored (or shared) config
cfcli lh config write lab/cage

# Pick one of the stored configs
cfcli lh config write

# Pipe YAML in from stdin
cat my_setup.yaml | cfcli lh config write
```

### Check

Compare the configuration on the Crazyflie with another one, base station by
base station.

```text
cfcli lh config check [<CONFIG> | -i <FILE>]
```

The configuration is chosen as for [write](#write), except that without one
`check` doesn't ask: it uses the configuration the selected swarm names.

Values are compared exactly, as the Crazyflie stores them. For each base
station, the geometry and the calibration are:

- `same`
- for geometry: how far the Crazyflie's is from the file's, `moved 4.1 cm,
  turned 0.62°`
- for calibration: `other base station` when the UIDs differ. The Crazyflie
  has seen another base station on that channel and taken its calibration,
  so the base station was probably replaced and the geometry may need a new
  estimate. `other values` when the UID is the same.
- `not on the Crazyflie` or `not in the file`

The command exits with 0 when the Crazyflie has the configuration in the
file and with **60** when it differs, so scripts can tell which Crazyflies
need a `write`.

With `--csv` the result is one row per base station:

```text
bs_id,geometry,moved_m,turned_deg,calibration,file_uid,cf_uid
0,same,,,same,3900185618,3900185618
1,differs,0.041,0.62,same,824402352,824402352
2,only_in_file,,,only_in_file,2148461594,
```

`geometry` and `calibration` are `same`, `differs`, `only_in_file`,
`only_on_cf` or `absent` (in neither).

#### Check Examples

```text
# Is the Crazyflie up to date?
cfcli lh config check -i my_setup.yaml

# Write only when it differs
cfcli lh config check lab/cage; [ $? -eq 60 ] && cfcli lh config write lab/cage
```

## Stored and shared configurations

cfcli keeps lighthouse configurations the way it keeps swarms: local ones in a
`lighthouse` folder next to the cfcli config, named `<config>`, and, when
signed in (`cfcli auth login`), shared ones on the server, named
`<org>/<config>`. A shared configuration has revisions, so when the base
stations are moved, everyone gets the new geometry, and the server's web
page shows what changed. With sync on (the default), commands check the
server for the latest version; with sync off, or without the server, they use
this computer's copy, and `pull` and `push` sync them, as for
[swarms](/docs/swarm.md#sync).

```text
# Store what a Crazyflie has (after estimating the geometry in cfclient)
cfcli lh config save lab/cage --name "The cage"

# ... or a file saved by the Crazyflie client
cfcli lh config import Lighthouse_Cage.yaml --id lab/cage --name "The cage"

# List them
$ cfcli lh config list
ID       | Name     | Base stations | Stored
---------+----------+---------------+-----------------------------
lab/cage | The cage | 4             | arc.bitcraze.io, revision 2

# Give it to a Crazyflie, or check one
cfcli lh config write lab/cage
cfcli lh config check lab/cage

# A file the Crazyflie client opens
cfcli lh config export lab/cage -o cage.yaml
```

`save` and `import` to an existing ID show what changes for each base station
and ask before replacing it (`--force` replaces without asking, and is needed
when not interactive). A configuration without any base station positions is
refused: estimate the geometry first. `base stations` in the list counts the
positioned ones; a Crazyflie also keeps the calibration of base stations it
has seen elsewhere.

`move` shares a local configuration (`move cage lab/cage`), takes a shared one
back (which deletes it on the server for everyone in the organization), or
renames one. Swarms that fly in it keep the old ID: `move` lists them and the
`cfcli swarm config lh` command that gives them the new one. A local swarm
can only name a configuration on this computer, and a shared swarm one in its
organization, so not every swarm can follow: taking a shared configuration
back, for example, leaves its shared swarms without it. The server refuses a
shared swarm that names another organization's configuration, so that
everyone who sees the swarm can see it.

`delete` deletes a configuration, a shared one on the server for everyone in
the organization. Before asking, it lists the swarms that fly in it: the
local ones and, for a shared configuration, the organization's shared swarms
(the server's list, or this computer's copies when the server can't be
reached). They keep naming it, so give them another one, or none with
`cfcli swarm config lh --clear`.

```text
$ cfcli lh config delete lab/cage
Swarms that fly in lighthouse config 'lab/cage': 'lab/flight-test'
? Delete lighthouse config 'lab/cage' on arc.bitcraze.io, for everyone in lab? Yes
Deleted lighthouse config 'lab/cage'
'lab/flight-test' still names it: give it another with 'cfcli swarm config lh <CONFIG> --swarm <SWARM>', or none with '--clear'
```

## Copy a Configuration Between Crazyflies

Store it from one Crazyflie and write it to the others (`save`, then `write`
or `swarm lh write`), or pipe `read` into `write`, overriding the `--uri` for
each:

```bash
cfcli --uri radio://0/80/2M/E7E7E7E7E7 lh config read \
  | cfcli --uri radio://0/80/2M/E7E7E7E7E8 lh config write
```
