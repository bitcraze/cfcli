//! The EEPROM config of a Crazyflie: radio channel, speed and address, and
//! the trims.

use anyhow::{bail, Context, Result};
use crazyflie_connection_server::memory::{EEPROMConfigMemory, MemoryType};
use crazyflie_connection_server::Crazyflie;

/// Read the EEPROM config. Its setters change the copy read, `commit()`
/// writes it.
pub async fn open(cf: &Crazyflie) -> Result<EEPROMConfigMemory> {
    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));
    if memories.len() != 1 {
        bail!("No EEPROMConfig memory found or more than one ({})", memories.len());
    }
    cf.memory.open_memory::<EEPROMConfigMemory>(memories[0]).await.context("Could not read the EEPROM config")
}

/// Write a new radio channel and address to the EEPROM config, then read
/// them back. `None` keeps what is there. The firmware only applies them at
/// boot.
pub async fn write_radio(cf: &Crazyflie, channel: Option<u8>, address: Option<[u8; 5]>) -> Result<()> {
    let mut eeprom = open(cf).await?;
    if let Some(channel) = channel {
        eeprom.set_radio_channel(channel)?;
    }
    if let Some(address) = address {
        eeprom.set_radio_address(address);
    }
    eeprom.commit().await?;

    // Opening it again reads it from the Crazyflie.
    let written = open(cf).await?;
    let (stored_channel, stored_address) = (written.get_radio_channel(), written.get_radio_address());
    if let Some(channel) = channel.filter(|channel| *channel != stored_channel) {
        bail!("the EEPROM config reads back channel {} after writing {}", stored_channel, channel);
    }
    if let Some(address) = address.filter(|address| address != &stored_address) {
        bail!(
            "the EEPROM config reads back address {} after writing {}",
            hex(&stored_address),
            hex(&address)
        );
    }
    Ok(())
}

/// Write a new radio channel to the EEPROM config and read it back.
pub async fn write_radio_channel(cf: &Crazyflie, channel: u8) -> Result<()> {
    write_radio(cf, Some(channel), None).await
}

fn hex(address: &[u8]) -> String {
    address.iter().map(|b| format!("{:02X}", b)).collect()
}
