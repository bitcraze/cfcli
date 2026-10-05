# Swarms

The **swarm** command keeps lists of Crazyflies, called swarms, and runs
commands on all the Crazyflies in one. One swarm is selected, and the swarm
commands act on the Crazyflies in it. Every Crazyflie in a swarm has a short
name (`CF-01`, `Rig`, ...) that is shown in the output and used to pick
Crazyflies.

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

CF    | URI                       | Description
------+---------------------------+-------------
CF-01 | radio:///80/2M/E7E7E7E701 |
Rig   | radio:///80/2M/E7E7E7E702 | Test bench
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
> Name for radio:///80/2M/E7E7E7E701: (CF-01) Alpha
Added Alpha radio:///80/2M/E7E7E7E701
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
Added Alpha radio:///80/2M/E7E7E7E704
```

A Crazyflie that is already in the swarm (same channel and address) is skipped.
That includes Crazyflies that still have the default address: give each one its
own address first, with `cfcli config set address=...`. A radio of 0 is
stored empty, meaning any Crazyradio, see [Any Crazyradio](#any-crazyradio).

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

Every `radio://0/` URI in an imported file is stored as `radio:///` (any
Crazyradio), so the swarm isn't tied to the first Crazyradio. URIs with another
radio are kept.

Export the selected swarm, or another one by ID, to stdout or to a file:

```bash
cfcli swarm config export
cfcli swarm config export lab -o lab.yaml
```

## Any Crazyradio

Leaving the radio in a URI empty means any Crazyradio:
`radio:///80/2M/E7E7E7E7E7` is channel 80, address E7E7E7E7E7, on whichever
Crazyradio cfcli picks. A number still means exactly that Crazyradio. An empty
host is how a URI asks for the default, the same way `file:///` means this
machine, and it needs no quoting in any shell:

```bash
cfcli -u radio:///80/2M/E7E7E7E7E7 platform info
```

* **One Crazyflie:** a selected (or `--uri`) URI without a radio uses the first
  Crazyradio that can be opened. A Crazyradio that another program holds, such
  as Swarmkeeper, is skipped.
* **A swarm:** the Crazyflies are spread over all Crazyradios that can be
  opened. Crazyflies on the same channel always share a Crazyradio, since two
  radios on one channel would collide, and channels less than 2 apart count as
  the same channel (a 2 Mbit/s channel is about 2 MHz wide). The channels with
  the most Crazyflies are placed first, each on the Crazyradio with the fewest
  Crazyflies so far. With a single Crazyradio everything runs on it.

A swarm on a single channel therefore always uses one Crazyradio; spread it
over several channels with [`swarm rechannel`](#spreading-a-swarm-over-channels)
to make use of more.

## Checking which Crazyflies answer

```bash
cfcli swarm scan
```

```text
CF    | URI                       | Radio | Online
------+---------------------------+-------+--------
CF-01 | radio:///80/2M/E7E7E7E701 | 0     | yes
Rig   | radio:///80/2M/E7E7E7E702 | 0     | no
1 of 2 Crazyflies answered
```

`Radio` is the Crazyradio each Crazyflie was checked on. This only sends a
packet to each Crazyflie, it doesn't connect. Use `--cf` and `--exclude`
(comma-separated names) to check some of them, and `--swarm <id>` for another
swarm. With `--csv` the output is `cf,uri,radio,online`.

## Running commands on the swarm

These commands work like the normal ones, on every Crazyflie in the swarm:

| Command | Does |
|---------|------|
| `cfcli swarm platform info` | Platform, firmware and CRTP protocol of each Crazyflie |
| `cfcli swarm platform reboot \| power-off \| sleep \| wakeup` | Reboot, power off, sleep or wake up each Crazyflie |
| `cfcli swarm param get \| set \| store \| clear` | Parameters on each Crazyflie |
| `cfcli swarm log print` | Log variables from each Crazyflie |
| `cfcli swarm deck list` | The decks on each Crazyflie |
| `cfcli swarm debug assert` | The assert info of each Crazyflie |

`--cf` and `--exclude` (comma-separated names) pick some of the Crazyflies,
and `--swarm <id>` runs on another swarm than the selected one:

```bash
cfcli swarm param set commander.enHighLevel=1 --cf CF-01,CF-02
cfcli swarm platform reboot --exclude Rig
```

Listings look like the normal ones, with a `CF` column in front:

```text
$ cfcli swarm param get stabilizer.estimator
CF    | Name                 | Access | Persistent | Default | Stored Value | Value
------+----------------------+--------+------------+---------+--------------+-------
CF-01 | stabilizer.estimator |   RW   |            |         |              | U8(2)
Rig   | stabilizer.estimator |   RW   |            |         |              | U8(2)
```

With `--csv` every row starts with the Crazyflie's name and URI
(`cf,uri,...`). Commands that only do something print one line per Crazyflie:

```text
$ cfcli swarm platform reboot
CF-01: rebooted
Rig: connection error: radio://0/80/2M/E7E7E7E702 doesn't answer
Error: some Crazyflies failed: 1 of 2 Crazyflies
```

Without names, `param get`, `set`, `store` and `clear` let you pick the
parameters from those of the first Crazyflie, and `set` asks for each value
once for all of them. The same goes for the variables of `log print`.

### Logging

`swarm log print --once` reads one sample from each Crazyflie: a row per
Crazyflie, a column per variable. It checks the batteries of a whole swarm in
one command:

```text
$ cfcli swarm log print pm.vbat,pm.state --once
CF    | pm.vbat  | pm.state
------+----------+----------
CF-01 | 4.196481 | 2
CF-02 | 4.117302 | 2
```

Without `--once` it logs from all the Crazyflies at the same time, a row per
sample with the Crazyflie in front (and its name and URI in front of each CSV
row), until stopped with Ctrl-C or `--timeout`:

```text
$ cfcli --timeout 3000 swarm log print pm.vbat,pm.state -p 500
CF    | Time (ms)  | pm.vbat      | pm.state
------+------------+--------------+--------------
CF-02 | 12960186   | 4.1595306    | 2
CF-01 | 12937940   | 4.1806455    | 2
```

A Crazyflie that can't start logging, or that drops out later, is reported on
stderr while the others go on. As for the normal `log print`, `--timeout` ends
it with exit code 0; the exit code rules below only apply when every
Crazyflie has stopped on its own.

### When some Crazyflies fail

A Crazyflie that can't be reached, or where the command fails, doesn't stop
the others. Its error is printed on stderr, prefixed with its name, and the
exit code tells what happened:

* every Crazyflie succeeded: 0;
* every Crazyflie failed in the same way: that failure's usual exit code, for
  instance 10 when none of them answer, or 20 for a parameter none of them
  have;
* otherwise: 50.

`reboot`, `power-off`, `sleep` and `wakeup` are sent without the Crazyflie
confirming them, so each Crazyflie is first checked to answer. A Crazyflie that
is switched off is reported as not answering rather than as done. A sleeping
Crazyflie still answers, so `wakeup` works.

### How it runs

The Crazyflies are spread over the Crazyradios the same way as for `swarm
scan`, and each Crazyradio handles up to 8 of them at a time. More Crazyradios,
and Crazyflies on more channels, make big swarms faster.

Crazyflies running the same firmware share their parameter and log TOCs. A
TOC that isn't in the cache yet is downloaded from one Crazyflie only; the
others wait and then use the cache. Run with `-n` (no TOC cache) and every
Crazyflie downloads its own.

## Spreading a swarm over channels

Crazyflies on the same channel always share a Crazyradio (see
[Any Crazyradio](#any-crazyradio)), so a swarm on one channel can't use more
than one Crazyradio. `swarm rechannel` moves the Crazyflies of a swarm onto
several channels. With one Crazyradio per channel, `--count` set to the number
of Crazyradios lets each one serve its own part of the swarm.

```bash
cfcli swarm rechannel --count 3            # 80, 78 and 76
cfcli swarm rechannel --channels 80,76,72  # these channels
```

`--count` starts at channel 80 and goes down in steps of 2: the lower channels
are more crowded, and channels less than 2 apart interfere at 2M. Only the
channel of each Crazyflie changes, not its address, datarate or radio.

Each channel gets an equal share of the Crazyflies. Crazyflies that are
already on one of the channels stay there, as far as its share allows, so as
few Crazyflies as possible are reprogrammed, and running the same command
again changes nothing. A swarm on 72, 76 and 80, moved with `--count 3`, only
reprograms the Crazyflies on 72 (to 78).

cfcli shows which Crazyflies move and asks before doing anything. `--dry-run`
stops after showing it, and `--yes` skips the question (needed when running
non-interactively):

```text
$ cfcli swarm rechannel --count 2
CF    | URI                       | From | To
------+---------------------------+------+----
CF-01 | radio:///90/2M/ABAD1DEA01 | 90   | 80
CF-02 | radio:///90/2M/ABAD1DEA02 | 90   | 78
2 of 3 Crazyflies move
? Reprogram 2 Crazyflies? (y/N)
```

The channel is stored in the EEPROM config of each Crazyflie, and the firmware
only reads it at boot. So each Crazyflie that moves is reprogrammed on its old
channel (the same as `cfcli config set channel=...`), rebooted, and then looked
for on its new channel. Its URI in the swarm file, and the selected URI if it
was this Crazyflie's, change only once it answers there. A summary shows what
happened to each one, and the exit code follows the same rules as the other
swarm commands.

If the command is interrupted, run it again. A Crazyflie that no longer
answers on its old channel but does on its new one only gets its URI updated.

Two Crazyflies on the same channel with the same address can't be told apart,
so a plan that would put them together is refused before anything is changed.
Crazyflies that still have the default address need their own address first,
with `cfcli config set address=...`.

## Using one Crazyflie from a swarm

`select --from-swarm` selects a Crazyflie from the selected swarm for all the
other cfcli commands. Give its name, or leave it out to pick one from a list:

```bash
cfcli select --from-swarm Rig
cfcli select --from-swarm
```

The URI is saved as it is written in the swarm. An empty radio is filled in
each time a command connects.

## Where swarms are stored

Swarms are files in a `swarms` folder next to the cfcli config file, on Linux
`~/.config/cf-cli/swarms/<id>.yaml`. `cfcli settings show` prints the folder
and the selected swarm. Use `import` and `export` rather than editing the files.

### File format

```yaml
name: Lab Crazyflies
description: The bench
units:
- uri: radio:///80/2M/E7E7E7E701
  name: CF-01
- uri: radio:///80/2M/E7E7E7E702
  name: Rig
  description: Test bench
```

`description` is optional, the rest is required. Fields cfcli doesn't know are
kept when it rewrites a file.
