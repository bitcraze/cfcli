# Connection server

> **Experimental.** The connection server, the `--connection` option and the
> `settings connection` command may change or go away in later releases.

A connection server holds the Crazyradios and keeps one connection per
Crazyflie. Several programs can then use the same Crazyflies at the same
time, for example Swarmkeeper monitoring a swarm while cfcli sets parameters,
and a server on one computer can serve programs on others. The server is
[crazyflie-connection-server](https://github.com/evoggy/crazyflie-connection-server).

```text
Usage: cfcli server <COMMAND>

Commands:
  run   Run a connection server (experimental): it holds this computer's Crazyradios and shares the Crazyflies between every program attached to it
  list  List the Crazyflies the server is connected to, and the programs attached to them
  help  Print this message or the help of the given subcommand(s)
```

## Where cfcli reaches the Crazyflies

| Connection | The Crazyradios are driven by |
|---|---|
| `auto` (default) | a connection server on this computer if one runs, otherwise cfcli itself (`direct`) |
| `direct` | cfcli itself |
| `local` | the connection server on this computer (`127.0.0.1:7777`) |
| `host`, `host:port` | a connection server on another computer |

Pick one per command with `--connection` or the `CRAZYFLIE_CONNECTION`
environment variable, or store it:

```bash
cfcli settings connection show
cfcli settings connection set lab-pc:7777
cfcli settings connection clear    # back to auto
```

Swarmkeeper serves its connections on `127.0.0.1:7777` while it runs, so in
`auto` cfcli uses the Crazyflies Swarmkeeper is connected to instead of
competing with it for the Crazyradios.

## Run a server

```bash
cfcli server run
```

The server opens every Crazyradio on this computer and listens on
`127.0.0.1:7777` until Ctrl-C.

| Option | Default | |
|---|---|---|
| `--listen` | `127.0.0.1:7777` | Address to listen on. `0.0.0.0:7777` takes clients from other computers |
| `--linger` | `10` | Seconds a Crazyflie stays connected after its last client is gone, so that consecutive commands reuse the connection |
| `--toc-cache DIR` | the cache directory, e.g. `~/.cache/cf-cli/server-toc-cache` | Where the server keeps the Crazyflies' TOCs, so that a restart doesn't download them again |

There is no authentication yet: anyone who reaches the port can fly the
Crazyflies. Listen on other addresses only on a trusted network or over a
tunnel.

## List what the server holds

```bash
cfcli server list
```

Lists the Crazyflies the server is connected to and the programs attached to
each of them.

## Commands that need direct mode

Most commands, swarm commands included, go through a server. The ones that
don't yet stop with exit code 10 when a server holds the Crazyradios, and say
so. Stop the server, or run the command with `--connection direct` and a
Crazyradio the server doesn't hold.
