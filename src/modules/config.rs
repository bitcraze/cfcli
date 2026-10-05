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

/// Write a new radio channel to the EEPROM config and read it back. The
/// firmware only applies it at boot.
pub async fn write_radio_channel(cf: &Crazyflie, channel: u8) -> Result<()> {
    let mut eeprom = open(cf).await?;
    eeprom.set_radio_channel(channel)?;
    eeprom.commit().await?;
    cf.memory.close_memory(eeprom).await?;

    // Opening it again reads it from the Crazyflie.
    let written = open(cf).await?;
    let stored = written.get_radio_channel();
    cf.memory.close_memory(written).await?;
    if stored != channel {
        bail!("the EEPROM config reads back channel {} after writing {}", stored, channel);
    }
    Ok(())
}
