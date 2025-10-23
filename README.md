# Crazyflie CLI

This is a command line interface (CLI) for the Bitcraze Crazyflie written in Rust. It's intended to be used
during development to easily log the console, set parameters and get logging variables. It's not
intended to be used from any scripts to make the Crazyflie do things, then using the
[Crazyflie python library](https://github.com/bitcraze/crazyflie-lib-python) is preferred.

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
  log      Access to the log subsystem
  param    Access to the parameter subsystem
  mem      Access to the memory subsystem
  config   Configure the Crazyflie (radio settings, etc)
  util     Various supporting utilities for the Crazyflie and its ecosystem
  scan     List the Crazyflies found while scanning (on the selected address)
  select   Scan for Crazyflies and select which one to save for later interactions
  console  Print the console text from a Crazyflie
  help     Print this message or the help of the given subcommand(s)

Options:
  -a, --address <ADDRESS>  Specify address [default: E7E7E7E7E7]
  -h, --help               Print help
  -V, --version            Print version
```

To use the CLI you must first select which URI to use, this is done by scanning for available Crazyflies 
and selecting the one you prefer.

```text
cfcli select
```

If you have a Crazyflie on a different address than the default you can specify it using the `-a` flag:

```text
cfcli -a E7E7E7E7E7 select 
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

Some commands support interactively selecting parameters, like the following commands:

```text
cfcli param get
cfcli param set
cfcli log print
cfcli mem display
```

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
