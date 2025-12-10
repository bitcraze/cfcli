use serde::{Deserialize, Serialize};

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
  pub fn from_yaml(path: String) -> Result<DeckConfig, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let cfg: DeckConfig = serde_yaml::from_str(&content)?;
    Ok(cfg)
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
    
    // Add name as fixed 15 byte array, zero terminated
    let name_bytes = self.name.as_bytes();
    let mut name_array = [0u8; 15];
    let copy_len = std::cmp::min(name_bytes.len(), 14); // Leave space for null terminator
    name_array[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
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