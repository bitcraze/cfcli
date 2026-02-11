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

It's not intended to be used for creating more advanced scripts or functionality, it's better to
use the the [Crazyflie python library](https://github.com/bitcraze/crazyflie-lib-python) for that.

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
  hlc       High-level commander operations (takeoff, land, go-to, trajectory, etc.)
  help      Print this message or the help of the given subcommand(s)

Options:
  -n, --no-toc-cache  Do not use TOC cache
  -d, --debug         Enable debug mode
  -h, --help          Print help
  -V, --version       Print version
```

To use the CLI you must first select which URI to use, this is done by scanning for available Crazyflies
and selecting the one you prefer.

```text
cfcli select
```

If you have a Crazyflie on a different address than the default you can specify it while scanning or selecting:

```text
cfcli select 0xE7E7E7E7E7
```

Now this URI will be used in all commands until a new one is selected. For instance a parameter
can be set using the following command:

```text
cfcli param set motorPowerSet.enable 1
```

And a log variable can be printed using the following command:

```text
cfcli log print stabilizer.roll 100
```

A release can be flashed with:

```text
cfcli bootload flash --release 2025.12
```

For a more indepth view on how to use the different commands, have a look at the documentation:

* [Bootloader](/docs/bootload.md)
* [Console](/docs/console.md)
* [High-Level Commander](/docs/high-level-commander.md)
* [Logging](/docs/logging.md)
* [Memory](/docs/memory.md)
* [Platform](/docs/platform.md)

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