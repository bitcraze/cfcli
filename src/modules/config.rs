//! The EEPROM config of a Crazyflie: radio channel, speed and address, and
//! the trims.

use anyhow::{bail, Result};
use crazyflie_lib::subsystems::memory::{EEPROMConfigMemory, MemoryType};
use crazyflie_lib::Crazyflie;

/// Open the EEPROM config. Hand it back with `cf.memory.close_memory()` to
/// open it again.
pub async fn open(cf: &Crazyflie) -> Result<EEPROMConfigMemory> {
    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));
    if memories.len() != 1 {
        bail!("No EEPROMConfig memory found or more than one ({})", memories.len());
    }
    match cf.memory.open_memory::<EEPROMConfigMemory>(memories[0].clone()).await {
        Some(Ok(eeprom)) => Ok(eeprom),
        Some(Err(e)) => bail!("Could not read the EEPROM config: {}", e),
        None => bail!("No EEPROM config memory found"),
    }
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
    cf.memory.close_memory(eeprom).await?;

    // Opening it again reads it from the Crazyflie.
    let written = open(cf).await?;
    let (stored_channel, stored_address) = (written.get_radio_channel(), *written.get_radio_address());
    cf.memory.close_memory(written).await?;
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
