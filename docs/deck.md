# Decks

The **deck** command shows the expansion decks that are attached to the
Crazyflie.

## Listing decks

```bash
cfcli deck list
```

This prints one line per attached deck with its board name, board revision and
serial number:

```text
Name           | Rev | Serial
---------------|-----|-------------------------
bcFlow2        | A   | 0DD07B8E0000002D
WiFi camera de | G   | 7E0048001250465753393020
```

For machine-readable output, add the global `--csv` flag:

```bash
cfcli --csv deck list
```

```text
name,revision,serial
bcFlow2,A,0DD07B8E0000002D
WiFi camera de,G,7E0048001250465753393020
```

## Where the information comes from

Decks are found through their identity memory, which is also visible in
`cfcli mem list`. There are two kinds, and both are listed:

* **`OneWire`** — decks with a passive 1-wire EEPROM. The name and the revision
  are stored as elements in that memory, and the serial number is the 1-wire
  ROM id of the memory chip (8 bytes).
* **`DeckCtrl`** — decks with a deck controller MCU. The name and the revision
  come from the info page at address 0 of the deck controller memory, and the
  serial number is the unique CPU id of the controller (12 bytes).

The serial number is reported by the Crazyflie together with the memory
listing, so it is available even for a deck whose memory can't be read. The
name and the revision require a read, and a deck that doesn't answer — or that
holds data cfcli can't make sense of, such as a 1-wire memory that fails its
CRC check — is still listed with its serial number and `?` in the fields that
couldn't be read. The reason is printed on stderr, so it doesn't disturb the
output of `--csv`.

Note that the name of a `DeckCtrl` deck is stored in a 15 byte field, so long
names are truncated on the deck itself when the configuration is written (see
`cfcli util deck-ctrl bingen`). `deck list` shows the name as it is stored.

A deck that has no identity memory at all is not listed — the Crazyflie has no
way of knowing it is there.
