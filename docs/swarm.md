# Swarms

The **swarm** command keeps lists of Crazyflies, called swarms. One swarm is
selected, and the swarm commands act on the Crazyflies in it. Every Crazyflie
in a swarm has a short name (`CF-01`, `Rig`, ...) that is shown in the output
and used to pick Crazyflies.

Swarms are stored in the same format as [Swarmkeeper](#file-format), so a swarm
file can be moved between the two with `import` and `export`.

## Managing swarms

Create a swarm and list the stored ones. The first swarm you create (or import)
is selected automatically:

```bash
cfcli swarm config create lab --name "Lab Crazyflies" --description "The bench"
cfcli swarm config list
```

```text
  | ID   | Name           | Crazyflies
--+------+----------------+------------
* | lab  | Lab Crazyflies | 3
  | show | Show swarm     | 49
```

The ID (`lab`) is what you type in commands. It is also the file name, so it
can only contain letters, digits, `-`, `_` and `.`. The name is only shown.

Select another swarm by ID, or run `select` without an ID to pick one from a
list. When running non-interactively, `select` without an ID prints the list
instead:

```bash
cfcli swarm config select show
cfcli swarm config select
```

Show the Crazyflies in the selected swarm, or in another one by ID:

```bash
cfcli swarm config show
cfcli swarm config show lab
```

```text
Swarm 'lab': Lab Crazyflies
The bench

CF    | URI                        | Description
------+----------------------------+-------------
CF-01 | radio://*/80/2M/E7E7E7E701 |
Rig   | radio://*/80/2M/E7E7E7E702 | Test bench
```

Delete a swarm with `cfcli swarm config delete <id>`, or leave the ID out to
pick it from a list. Since a deleted swarm can't be brought back, cfcli asks
before deleting it, unless it runs non-interactively.

`list` and `show` support the global `--csv` flag.

## Adding and removing Crazyflies

Add Crazyflies by URI. When you add a single Crazyflie, give it a name with
`--name` or type one at the prompt (pressing Enter takes the next free `CF-NN`).
Crazyflies added several at once get the next free names, `CF-01`, `CF-02`, ...:

```bash
cfcli swarm config add radio://0/80/2M/E7E7E7E702 --name Rig --description "Test bench"
cfcli swarm config add radio://0/80/2M/E7E7E7E701
cfcli swarm config add radio://0/80/2M/E7E7E7E703 radio://0/80/2M/E7E7E7E704
```

```text
> Name for radio://*/80/2M/E7E7E7E701: (CF-01) Alpha
Added Alpha radio://*/80/2M/E7E7E7E701
```

When running non-interactively, adding a single Crazyflie needs `--name`.

Or scan for Crazyflies and add every one that isn't in the swarm yet. This
scans the given address, or the scan addresses in `cfcli settings`:

```bash
cfcli swarm config add --scan E7E7E7E701
```

Or connect a Crazyflie over USB and add the radio URI from its config. This
adds one Crazyflie at a time, so connect only the one to add; with more than
one on USB the command stops without adding anything. As with a single URI,
give the name with `--name` or type it at the prompt:

```bash
cfcli swarm config add --from-usb --name Alpha
```

```text
Found Crazyflie on USB: usb://2F0040001347343439303733
Added Alpha radio://*/80/2M/E7E7E7E704
```

A Crazyflie that is already in the swarm (same channel and address) is skipped.
That includes Crazyflies that still have the default address: give each one its
own address first, with `cfcli config set address=...`. A radio of 0 in the URI
is stored as `*`, see [Any Crazyradio](#any-crazyradio).

Rename a Crazyflie, or remove Crazyflies by name or URI. Without arguments,
`rename` lets you pick the Crazyflie from a list and asks for the new name, and
`remove` lists the Crazyflies to mark the ones to remove (Space marks, Enter
removes):

```bash
cfcli swarm config rename CF-01 Alpha
cfcli swarm config rename
cfcli swarm config remove CF-03 Rig
cfcli swarm config remove
```

Names are matched ignoring case, must be unique in the swarm and can't contain
a comma. `add`, `remove` and `rename` change the selected swarm, or the one
given with `--swarm <id>`.

## Import and export

Import one or more swarm files. The file name becomes the ID, unless you pick
one with `--id` (only when importing a single file). An existing swarm is only
replaced with `--force`:

```bash
cfcli swarm config import ~/Documents/Swarmkeeper/swarms/*.yaml
cfcli swarm config import set-1.yaml --id lab-set
```

Every `radio://0/` URI in an imported file is stored as `radio://*/`, so the
swarm isn't tied to the first Crazyradio. URIs with another radio are kept.

Export the selected swarm, or another one by ID, to stdout or to a file:

```bash
cfcli swarm config export
cfcli swarm config export lab -o lab.yaml
```

## Any Crazyradio

A `*` as the radio in a URI means any Crazyradio:
`radio://*/80/2M/E7E7E7E7E7` is channel 80, address E7E7E7E7E7, on whichever
Crazyradio cfcli picks. A number still means exactly that Crazyradio.

* **One Crazyflie:** a selected (or `--uri`) URI with `*` uses the first
  Crazyradio that can be opened. A Crazyradio that another program holds, such
  as Swarmkeeper, is skipped.
* **A swarm:** the Crazyflies are spread over all Crazyradios that can be
  opened. Crazyflies on the same channel always share a Crazyradio, since two
  radios on one channel would collide, and channels less than 2 apart count as
  the same channel (a 2 Mbit/s channel is about 2 MHz wide). The channels with
  the most Crazyflies are placed first, each on the Crazyradio with the fewest
  Crazyflies so far. With a single Crazyradio everything runs on it.

A swarm on a single channel therefore always uses one Crazyradio; spread it
over several channels to make use of more.

In zsh (and bash with `failglob`) the `*` has to be quoted, or the shell tries
to expand it as a file name:

```bash
cfcli -u 'radio://*/80/2M/E7E7E7E7E7' platform info
```

## Checking which Crazyflies answer

```bash
cfcli swarm scan
```

```text
CF    | URI                        | Radio | Online
------+----------------------------+-------+--------
CF-01 | radio://*/80/2M/E7E7E7E701 | 0     | yes
Rig   | radio://*/80/2M/E7E7E7E702 | 0     | no
1 of 2 Crazyflies answered
```

`Radio` is the Crazyradio each Crazyflie was checked on. This only sends a
packet to each Crazyflie, it doesn't connect. Use `--cf` and `--exclude`
(comma-separated names) to check some of them, and `--swarm <id>` for another
swarm. With `--csv` the output is `cf,uri,radio,online`.

## Using one Crazyflie from a swarm

`select --from-swarm` selects a Crazyflie from the selected swarm for all the
other cfcli commands. Give its name, or leave it out to pick one from a list:

```bash
cfcli select --from-swarm Rig
cfcli select --from-swarm
```

The URI is saved as it is written in the swarm. A `*` is resolved each time a
command connects.

## Where swarms are stored

Swarms are files in a `swarms` folder next to the cfcli config file, on Linux
`~/.config/cf-cli/swarms/<id>.yaml`. `cfcli settings show` prints the folder
and the selected swarm. Use `import` and `export` rather than editing the files.

### File format

```yaml
name: Lab Crazyflies
description: The bench
units:
- uri: radio://*/80/2M/E7E7E7E701
  name: CF-01
- uri: radio://*/80/2M/E7E7E7E702
  name: Rig
  description: Test bench
```

`description` is optional, the rest is required. Fields cfcli doesn't know are
kept when it rewrites a file.
