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

    // Zero pad after header
    while bytes.len() < 0x20 {
      bytes.push(0);
    }

    // Terminate the partitions with one of zero size
    bytes.push(0);
    bytes.push(0);

    bytes
  }
}