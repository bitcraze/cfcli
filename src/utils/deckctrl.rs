use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Size of the product name field in the deck info page. The last byte is
/// reserved for the null terminator, so [`NAME_MAX_CHARS`] characters fit.
/// See the "Deck Information Format" table in the crazyflie-firmware docs
/// (`docs/functional-areas/deckctrl_protocol.md`).
pub const NAME_FIELD_LEN: usize = 15;

/// Longest product name that can be stored in the deck info page.
pub const NAME_MAX_CHARS: usize = NAME_FIELD_LEN - 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Partition {
  pub size: u16,
  pub id: u8,
  pub data: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeckConfig {
  pub version_major: u8,
  pub version_minor: u8,
  pub vid: u8,
  pub pid: u8,
  pub rev: char,
  pub name: String,
  pub manufactured: Option<String>,
  pub partitions: Vec<Partition>,
}

impl DeckConfig {
  pub fn from_yaml(path: String) -> Result<DeckConfig, anyhow::Error> {
    let content = std::fs::read_to_string(path)?;
    let cfg: DeckConfig = serde_yaml::from_str(&content)?;
    Ok(cfg)
  }

  /// The name as it will be stored in the info page: at most
  /// [`NAME_MAX_CHARS`] bytes, backed off to a character boundary so the
  /// field is always null terminated and stays valid UTF-8.
  pub fn stored_name(&self) -> &str {
    let mut end = std::cmp::min(NAME_MAX_CHARS, self.name.len());
    while !self.name.is_char_boundary(end) {
      end -= 1;
    }
    &self.name[..end]
  }

  /// A warning if [`DeckConfig::to_bytes`] can't store the name as written,
  /// `None` if it fits. Reported to the user rather than rejected, so a
  /// configuration that was already flashed still works.
  pub fn name_warning(&self) -> Option<String> {
    let stored_name = self.stored_name();
    if stored_name == self.name {
      return None;
    }

    Some(format!(
      "name \"{}\" is too long and will be stored as \"{}\" - the info page name \
       field holds {} characters plus a null terminator",
      self.name, stored_name, NAME_MAX_CHARS
    ))
  }

  pub fn to_bytes(&self) -> Vec<u8> {
    let mut bytes = Vec::new();
    // Add magic
    bytes.push(0xBC);
    bytes.push(0xDC);
    bytes.push(self.version_major);
    bytes.push(self.version_minor);
    bytes.push(self.vid);
    bytes.push(self.pid);
    
    // Add revision string length and bytes
    bytes.push(self.rev as u8);
    
    // Add name as fixed size array, zero terminated
    let name_bytes = self.stored_name().as_bytes();
    let mut name_array = [0u8; NAME_FIELD_LEN];
    name_array[..name_bytes.len()].copy_from_slice(name_bytes);
    bytes.extend_from_slice(&name_array);

    if let Some(mfg) = &self.manufactured {
      // Parse date YYYY-MM-DD and encode as 1 byte year (offset from 2000) + 1 byte month + 1 byte day
      let parts: Vec<&str> = mfg.split('-').collect();
      if parts.len() == 3 {
        let year: u16 = parts[0].parse().unwrap_or(0);
        let month: u8 = parts[1].parse().unwrap_or(0);
        let day: u8 = parts[2].parse().unwrap_or(0);
        bytes.push((year - 2000) as u8);
        bytes.push(month);
        bytes.push(day);
      }
    }

    // Zero pad after header
    while bytes.len() < 0x1F {
      bytes.push(0);
    }

    // Calculate checksum: sum of first 0x1F bytes, then write value at 0x1F that makes total sum 0
    let sum: u8 = bytes.iter().take(0x1F).fold(0u8, |acc, &b| acc.wrapping_add(b));
    let checksum = (0u8).wrapping_sub(sum);
    bytes.push(checksum);

    // Add partitions
    for partition in &self.partitions {
      bytes.push((partition.size & 0xFF) as u8);
      bytes.push(((partition.size >> 8) & 0xFF) as u8);
      bytes.push(partition.id);
      bytes.extend_from_slice(&partition.data);
    }

    // Terminate the partitions with one of zero size
    bytes.push(0);
    bytes.push(0);

    bytes
  }
}