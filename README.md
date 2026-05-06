# Crazyflie CLI

This is a command line interface (CLI) for the Bitcraze Crazyflie written in Rust. It's intended to be used
during development to quickly access various subsystems in the Crazyflie and supports features like:

* Flash firmware (nRF51/STM32F4 as well as decks)
* Get console output
* Log variables
* Get and set parameters
* Read and write (as well as display and erase some) memories
* Configure radio settings like channel, address and speed
* Turn the platform on/off or put it to sleep/wake it up
* Run stability tests with the Crazyflie
* High-level commander (takeoff, land, go-to, trajectories)
* AI-agent and scripting friendly (CSV output, classified exit codes, non-interactive mode, command timeouts)

It's not intended to be used for creating more advanced scripts or functionality, it's better to
use the the new [Crazyflie python library (v2)](https://github.com/bitcraze/crazyflie-lib-python-v2) for that.

## Installation

### Standalone .deb package

Download latest released `.deb` file from [GitHub Releases](https://github.com/evoggy/cfcli/releases) and install using:

```bash
sudo apt install ./cfcli_x.y.z_amd64.deb
```

This will also add the APT so you can install updates when they are released.

### APT repository (Debian/Ubuntu)

```bash
# Add repository
curl -fsSL https://evoggy.github.io/cfcli/cfcli-repo.gpg.key | sudo gpg --dearmor -o /usr/share/keyrings/cfcli-archive-keyring.gpg
echo "deb [arch=amd64,arm64 signed-by=/usr/share/keyrings/cfcli-archive-keyring.gpg] https://evoggy.github.io/cfcli stable main" | sudo tee /etc/apt/sources.list.d/cfcli.list

# Install
sudo apt update
sudo apt install cfcli
```

### Homebrew (OSX and Linux)

To add and install run the following:

```bash
brew tap evoggy/cfcli
brew install cfcli
```

To update run:

```bash
brew upgrade cfcli
```

## Usage

To see how to use the CLI type ```cfcli``` in your terminal and you will get the following help message:

```text
Crazyflie command-line client

Usage: cfcli [OPTIONS] <COMMAND>

Commands:
  log       Access to the log subsystem
  param     Access to the parameter subsystem
  mem       Access to the memory subsystem
  config    Configure the Crazyflie (radio settings, etc)
  util      Various supporting utilities for the Crazyflie and its ecosystem
  bootload  Bootload the Crazyflie and decks
  test      Run tests with the Crazyflie
  platform  Access platform functionality
  scan      List the Crazyflies found while scanning (on the selected address)
  select    Scan for Crazyflies and select which one to save for later interactions
  console   Print the console text from a Crazyflie
  settings  Local CLI settings (scan addresses, timeout, etc.)
  loco      Loco Positioning System
  hlc       High-level commander operations (takeoff, land, go-to, trajectory, etc.)
  cr        Crazyradio operations (sniffer, etc.)
  debug     Debugging utilities (assert info dumps, etc.)
  help      Print this message or the help of the given subcommand(s)

Options:
  -n, --no-toc-cache       Do not use TOC cache
  -d, --debug              Enable debug mode
  -u, --uri <URI>          Override the URI to connect to (instead of using the config file)
  -p, --preserve-console   Preserve console output across connections, printed when the 'console' command is run
      --timeout <TIMEOUT>  Timeout in milliseconds for the command
      --non-interactive    Disable interactive prompts (auto-set when stdin is not a TTY)
      --csv                Emit machine-readable CSV (for read commands that support it)
  -h, --help               Print help
  -V, --version            Print version

Exit codes:
   0  success
   1  unspecified error
   2  usage / argument error (clap)
  10  connection failure (no Crazyflie found, link error, disconnected)
  20  resource not found (param/log/memory by name, release name)
  30  invalid value (range, type, malformed input)
  40  --timeout expired on a bounded command
```

To use the CLI you must first select which URI to use, this is done by scanning for available Crazyflies
and selecting the one you prefer.

```text
cfcli select
```

If you have a Crazyflie on a different address than the default you can specify it while scanning or selecting:

```text
cfcli select E7E7E7E7E7
```

You can also configure persistent scan addresses and a connection timeout using the `settings` command:

```text
cfcli settings address add E7E7E7E701
cfcli settings timeout set 2000
```

Now this URI will be used in all commands until a new one is selected. You can also override the
selected URI for a single command using the `--uri` flag:

```text
cfcli --uri radio://0/80/2M/E7E7E7E7E7 console
```

You can preserve console output across connections using the `-p` flag. Console data is accumulated
during each connection and printed when the `console` command is run:

```text
cfcli -p param set motorPowerSet.enable 1
cfcli console
```

For instance a parameter
can be set using the following command:

```text
cfcli param set motorPowerSet.enable 1
```

And a log variable can be printed using the following command:

```text
cfcli log print stabilizer.roll -p 100
```

A release can be flashed with:

```text
cfcli bootload flash --release 2025.12
```

For a more indepth view on how to use the different commands, have a look at the documentation:

* [Bootloader](/docs/bootload.md)
* [Console](/docs/console.md)
* [High-Level Commander](/docs/high-level-commander.md)
* [Loco Positioning System](/docs/loco.md)
* [Logging](/docs/logging.md)
* [Memory](/docs/memory.md)
* [Parameters](/docs/parameters.md)
* [Platform](/docs/platform.md)
* [Select](/docs/select.md)
* [Settings](/docs/settings.md)
* [Crazyradio](/docs/crazyradio.md)
* [Debug](/docs/debug.md)
* [Test](/docs/test.md)

## Scripting / AI agent usage

When `cfcli` is driven from a script or from an AI agent (rather than typed at a
prompt) a few flags make the output predictable:

* `--non-interactive` — disable interactive `Select`/`MultiSelect` pickers.
  Auto-enabled when stdin isn't a TTY. Missing required arguments now produce
  a clear error (exit code 30) instead of hanging waiting for keyboard input.
* `--timeout <ms>` — global wall-clock cap for the whole command. For
  *streaming* commands (`console`, `log print`, `cr sniff`) the timer is the
  intended way to stop them and the command exits **0**. For all other
  commands a timeout means the command got stuck and the command exits **40**.
* `--csv` — machine-readable CSV output for the read commands (`scan`,
  `param list`/`get`, `log list`/`print`, `mem list`, `platform info`). Other
  commands ignore the flag.

Exit codes:

| Code | Meaning                                                        |
|------|----------------------------------------------------------------|
| 0    | Success                                                        |
| 1    | Unspecified error                                              |
| 2    | Usage / argument error (from clap)                             |
| 10   | Connection failure (no Crazyflie found, link error, etc.)      |
| 20   | Resource not found (param/log/memory by name, release name)    |
| 30   | Invalid value (range, type, malformed input)                   |
| 40   | `--timeout` expired on a bounded command                       |

Worked example — read one parameter into a shell variable:

```bash
value=$(cfcli --non-interactive --timeout 5000 --csv \
        param get motorPowerSet.enable \
        | tail -n +2 | awk -F, '{print $6}')
```

Watch the console for 3 seconds then stop cleanly:

```bash
cfcli --timeout 3000 console
```

The same information is available from `cfcli --help` on every install.

## Development

If you would like to run it from source use the following command:

```text
git clone https://github.com/evoggy/cfcli.git
cd cfcli
cargo run -- <args>
```

For example:

```text
cargo run -- select
```

### Cross compilation

Compilation for linux on other platform is possible using the [cross](https://github.com/cross-rs/cross) tool.

It requires both `cross` and [`docker` or `podman`] to be installed.

```bash
# Install podman or docker ...
# To install cross
cargo install cross

# To compile for ARM64
cross build --target aarch64-unknown-linux-gnu

# The resulting executable is now in ./target/aarch64-unknown-linux-gnu/debug/cfcli
```
