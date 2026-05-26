# Migrating to the Bitcraze repositories

`cfcli` moved from `github.com/evoggy/cfcli` to **`github.com/bitcraze/cfcli`**, and
its package distribution moved to Bitcraze-hosted repositories. GitHub redirects
the old git and release-download URLs automatically, so most things keep working —
but the **APT repository URL changed** and is not redirected, so existing apt
installations need a one-time fix.

## APT (Debian/Ubuntu)

The repository moved from `https://evoggy.github.io/cfcli` to
**`https://packages.bitcraze.io/apt`**. The signing key is unchanged, so no
re-import of the key is required — only the source list changes.

Remove the old source and add the new one:

```bash
# Remove the old repository configuration
sudo rm -f /etc/apt/sources.list.d/cfcli.list

# Add the shared Bitcraze repository
curl -fsSL https://packages.bitcraze.io/apt/bitcraze.gpg.key \
  | sudo gpg --dearmor -o /usr/share/keyrings/bitcraze-archive-keyring.gpg
echo "deb [arch=amd64,arm64 signed-by=/usr/share/keyrings/bitcraze-archive-keyring.gpg] https://packages.bitcraze.io/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/bitcraze.list

sudo apt update
```

Reinstalling the latest `.deb` (or any future Bitcraze `.deb`) also configures the
new repository automatically, so this manual step is only needed for machines that
were set up against the old URL.

## Homebrew

The tap moved to the shared `bitcraze/tap`. The old tap keeps working via GitHub's
redirect, but switching is recommended:

```bash
brew untap evoggy/cfcli
brew tap bitcraze/tap
brew upgrade cfcli
```

## Building from source

```bash
git clone https://github.com/bitcraze/cfcli.git
```

Old `evoggy/cfcli` clone URLs continue to redirect, but updating your `origin`
remote is recommended:

```bash
git remote set-url origin https://github.com/bitcraze/cfcli.git
```
