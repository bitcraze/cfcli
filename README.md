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

It's not intended to be used for creating more advanced scripts or functionality, it's better to
use the the [Crazyflie python library](https://github.com/bitcraze/crazyflie-lib-python) for that.

## Installation

If you would like to install the cli for general use use the following command:

```text
cargo install cfcli
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
* [Logging](/docs/logging.md)
* [Console](/docs/console.md)
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
