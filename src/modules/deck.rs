//! Deck listing for cfcli
//!
//! Lists the decks attached to the Crazyflie with their board revision and
//! serial number.
//!
//! Decks are found through their identity memory, of which there are two
//! kinds:
//!
//! * `OneWire` - the classic passive 1-wire EEPROM. The board name and the
//!   revision are stored as elements in the memory, the serial number is the
//!   1-wire ROM id of the memory chip.
//! * `DeckCtrl` - decks with a deck controller MCU. The board name and the
//!   revision come from the 32 byte info page at address 0, the serial number
//!   is the unique CPU id of the controller.
//!
//! Both kinds report the serial number in the memory info packet, so it is
//! available from `MemoryDevice::serial` without reading the memory itself.
//! The name and revision on the other hand require a read, and a deck that
//! doesn't answer (or holds a memory we can't make sense of) is still listed
//! with its serial number and `?` for the fields we couldn't read.

use anyhow::{bail, Result};
use crazyflie_lib::{
    subsystems::memory::{MemoryDevice, MemoryType, OwMemory, RawMemory},
    Crazyflie,
};

use crate::utils::display::{csv_row, print_table};

/// Magic marking a valid DeckCtrl info page, stored big endian (0xBCDC).
const DECKCTRL_MAGIC: [u8; 2] = [0xBC, 0xDC];
/// Size of the DeckCtrl info page, checksum byte included.
const DECKCTRL_INFO_LEN: usize = 0x20;
/// Board revision, a single ASCII character.
const DECKCTRL_REV_OFFSET: usize = 6;
/// Product name, null terminated.
const DECKCTRL_NAME_OFFSET: usize = 7;
const DECKCTRL_NAME_LEN: usize = 15;

/// Shown for a field the deck didn't give us.
const UNKNOWN: &str = "?";

/// One deck as presented by `deck list`.
struct Deck {
    name: String,
    revision: String,
    serial: String,
}

impl Deck {
    /// A deck we know is there (it has a memory) but haven't read yet.
    fn unread(device: &MemoryDevice) -> Self {
        Self {
            name: UNKNOWN.to_string(),
            revision: UNKNOWN.to_string(),
            serial: match &device.serial {
                Some(serial) => serial.iter().map(|b| format!("{:02X}", b)).collect(),
                None => UNKNOWN.to_string(),
            },
        }
    }
}

/// Print the decks attached to the Crazyflie.
pub async fn list(cf: &Crazyflie, csv: bool) -> Result<()> {
    let devices: Vec<MemoryDevice> = cf
        .memory
        .get_memories(None)
        .into_iter()
        .filter(|m| matches!(m.memory_type, MemoryType::OneWire | MemoryType::DeckCtrl))
        .cloned()
        .collect();

    let mut decks = Vec::with_capacity(devices.len());
    for device in &devices {
        decks.push(match device.memory_type {
            MemoryType::DeckCtrl => read_deckctrl_deck(cf, device).await,
            _ => read_ow_deck(cf, device).await,
        });
    }

    if csv {
        csv_row(&["name", "revision", "serial"]);
        for deck in &decks {
            csv_row(&[&deck.name, &deck.revision, &deck.serial]);
        }
        return Ok(());
    }

    if decks.is_empty() {
        println!("No decks found");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = decks
        .iter()
        .map(|deck| vec![deck.name.clone(), deck.revision.clone(), deck.serial.clone()])
        .collect();
    print_table(&["Name", "Rev", "Serial"], &rows);

    Ok(())
}

/// Read name and revision from a 1-wire deck memory.
async fn read_ow_deck(cf: &Crazyflie, device: &MemoryDevice) -> Deck {
    let mut deck = Deck::unread(device);

    let ow = match cf.memory.open_memory::<OwMemory>(device.clone()).await {
        Some(Ok(ow)) => ow,
        Some(Err(e)) => {
            warn(device, &format!("{}", e));
            return deck;
        }
        None => {
            warn(device, "memory not found");
            return deck;
        }
    };

    if let Some(name) = ow.elements().get("boardName") {
        deck.name = name.clone();
    }
    if let Some(revision) = ow.elements().get("revision") {
        deck.revision = revision.clone();
    }

    deck
}

/// Read name and revision from the info page of a deck-controller deck.
async fn read_deckctrl_deck(cf: &Crazyflie, device: &MemoryDevice) -> Deck {
    let mut deck = Deck::unread(device);

    let raw = match cf.memory.open_memory::<RawMemory>(device.clone()).await {
        Some(Ok(raw)) => raw,
        Some(Err(e)) => {
            warn(device, &format!("{}", e));
            return deck;
        }
        None => {
            warn(device, "memory not found");
            return deck;
        }
    };

    let info = match raw.read(0, DECKCTRL_INFO_LEN).await {
        Ok(info) => info,
        Err(e) => {
            warn(device, &format!("{}", e));
            return deck;
        }
    };

    match parse_deckctrl_info(&info) {
        Ok((name, revision)) => {
            deck.name = name;
            deck.revision = revision;
        }
        Err(e) => warn(device, &format!("{}", e)),
    }

    deck
}

/// Pull the board name and revision out of a DeckCtrl info page.
fn parse_deckctrl_info(info: &[u8]) -> Result<(String, String)> {
    if info.len() < DECKCTRL_INFO_LEN {
        bail!("short info page ({} bytes)", info.len());
    }
    if info[..DECKCTRL_MAGIC.len()] != DECKCTRL_MAGIC {
        bail!("invalid magic 0x{:02X}{:02X}", info[0], info[1]);
    }

    let revision = match info[DECKCTRL_REV_OFFSET] {
        rev if rev.is_ascii_graphic() => (rev as char).to_string(),
        _ => UNKNOWN.to_string(),
    };

    let name_bytes = &info[DECKCTRL_NAME_OFFSET..DECKCTRL_NAME_OFFSET + DECKCTRL_NAME_LEN];
    let name_bytes = match name_bytes.iter().position(|&b| b == 0) {
        Some(end) => &name_bytes[..end],
        None => name_bytes,
    };
    let name = String::from_utf8_lossy(name_bytes).trim().to_string();
    let name = if name.is_empty() { UNKNOWN.to_string() } else { name };

    Ok((name, revision))
}

/// Report a deck we couldn't fully read on stderr, so that stdout stays
/// parsable (the deck is still listed, with `?` for the missing fields).
fn warn(device: &MemoryDevice, message: &str) {
    eprintln!(
        "Warning: could not read {} memory ID={}: {}",
        device.memory_type, device.memory_id, message
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid info page for a deck named "Test deck", revision B.
    fn info_page() -> Vec<u8> {
        let mut info = vec![0u8; DECKCTRL_INFO_LEN];
        info[0] = 0xBC;
        info[1] = 0xDC;
        info[DECKCTRL_REV_OFFSET] = b'B';
        let name = b"Test deck";
        info[DECKCTRL_NAME_OFFSET..DECKCTRL_NAME_OFFSET + name.len()].copy_from_slice(name);
        info
    }

    #[test]
    fn parses_name_and_revision() {
        let (name, revision) = parse_deckctrl_info(&info_page()).unwrap();
        assert_eq!(name, "Test deck");
        assert_eq!(revision, "B");
    }

    #[test]
    fn name_filling_the_field_has_no_terminator() {
        let mut info = info_page();
        let name = b"123456789012345";
        info[DECKCTRL_NAME_OFFSET..DECKCTRL_NAME_OFFSET + DECKCTRL_NAME_LEN].copy_from_slice(name);
        assert_eq!(parse_deckctrl_info(&info).unwrap().0, "123456789012345");
    }

    #[test]
    fn unprogrammed_fields_are_unknown() {
        let mut info = info_page();
        info[DECKCTRL_REV_OFFSET] = 0xFF;
        info[DECKCTRL_NAME_OFFSET..DECKCTRL_NAME_OFFSET + DECKCTRL_NAME_LEN].fill(0);
        let (name, revision) = parse_deckctrl_info(&info).unwrap();
        assert_eq!(name, UNKNOWN);
        assert_eq!(revision, UNKNOWN);
    }

    #[test]
    fn rejects_bad_magic_and_short_pages() {
        let mut info = info_page();
        info[1] = 0x00;
        assert!(parse_deckctrl_info(&info).is_err());
        assert!(parse_deckctrl_info(&info_page()[..8]).is_err());
    }
}
