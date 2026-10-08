# Signing in

The `auth` command signs cfcli in to the server that shares swarms and
lighthouse configurations between cfcli, Swarmkeeper and the people you fly
with. Signing in is optional:
everything else in cfcli works without it.

```text
Usage: cfcli auth <COMMAND>

Commands:
  login   Sign in through the browser; cfcli gets its key by itself
  logout  Sign out and revoke cfcli's key
  status  Show who cfcli is signed in as
  help    Print this message or the help of the given subcommand(s)
```

## Sign in

```bash
cfcli auth login
```

cfcli opens the sign-in page in your browser. Sign in with GitHub or Google if
you aren't already, and allow "cfcli on <computer>". The browser hands the
key back to cfcli by itself; there is nothing to copy. The page then says you
can close the tab.

If the browser doesn't open, cfcli prints the link to open instead. Open it in
a browser on the same computer: the browser returns the key to cfcli over
`127.0.0.1`. `--no-browser` only prints the link.

`--server` signs in to another server, such as one on your own network:

```bash
cfcli auth login --server http://lab-server:3000
```

Signing in again replaces the previous sign-in and revokes its key.

## Show who cfcli is signed in as

```bash
cfcli auth status
```

Prints the server, your name, the key's name and the organizations your swarms
and lighthouse configurations are shared in. When cfcli isn't signed in, or
the key no longer works, it says so and exits with code 20.

## Sign out

```bash
cfcli auth logout
```

Revokes cfcli's key on the server and forgets it.

## The key

cfcli signs in with an API key of its own, named "cfcli on &lt;computer&gt;". It
reads and uploads swarms and lighthouse configurations as you in the
organizations you are a member of. It
is listed on the server's API keys page, where you can revoke it; that signs
cfcli out too.

The key is stored in `credentials.json` in the cfcli config folder (next to
the `swarms` folder, see `cfcli settings show`), readable only by you.
