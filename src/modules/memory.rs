use anyhow::{anyhow, bail, Context, Result};
use tabled::Tabled;
use crazyflie_connection_server::memory::{EEPROMConfigMemory, MemoryInfo, MemoryType, OwMemory, RawMemory};
use crazyflie_connection_server::Crazyflie;
use crazyflie_lib::subsystems::memory::DeckMemory;

fn print_eeprom_info(eeprom: &EEPROMConfigMemory) {
    let speed = match eeprom.get_radio_speed() {
        0 => "250 kbps".to_string(),
        1 => "1 Mbps".to_string(),
        2 => "2 Mbps".to_string(),
        other => format!("unknown ({})", other),
    };
    println!("EEPROM Config:");
    println!("  Radio Channel: {}", eeprom.get_radio_channel());
    println!("  Radio Speed: {}", speed);
    println!("  Pitch Trim: {:.4}", eeprom.get_pitch_trim());
    println!("  Roll Trim: {:.4}", eeprom.get_roll_trim());
    println!("  Radio Address: 0x{}", eeprom.get_radio_address().iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(""));
}

fn print_ow_info(ow: &OwMemory) {
    println!("OneWire Memory:");
    println!("  Used Pins: 0x{:04X}", ow.used_pins());
    println!("  Pin assignments:");
      for i in 0..16 {
        let pin_names = [
          "PC11", "PC10", "PB7", "PB6", "PB8", "PB5", "PB4", "PC12",
          "PA2", "PA3", "PA5", "PA6", "PA7", "P0.11", "P0.12", "P0.08"
        ];

        if (ow.used_pins() & (1 << i)) != 0 {
          let drive_level = if (ow.used_pins() & (1 << (i + 16))) != 0 { "High" } else { "Low" };
          println!("    {}: {} drive", pin_names[i], drive_level);
        }
      }
    println!("  VID: 0x{:02X}", ow.vid());
    println!("  PID: 0x{:02X}", ow.pid());
    println!("  Elements:");
    for (key, value) in ow.elements() {
        println!("    {}: {}", key, value);
    }
}

pub const DECK_CTRL_DFU_STATUS_IN_DFU_MODE: u8 = 1 << 0;
pub const DECK_CTRL_DFU_STATUS_CAN_ENABLE_DFU: u8 = 1 << 1;

pub struct DeckCtrlDfuHeader {
    pub version: u8,
    pub deck_ctrl_count: u8,
    pub status: u8,
}

/// The DeckCtrl DFU header: the first 4 bytes of the memory.
pub fn parse_deck_ctrl_dfu_header(header: &[u8]) -> Result<DeckCtrlDfuHeader> {
    if header.len() < 3 {
        bail!("short DeckCtrl DFU header ({} bytes)", header.len());
    }
    Ok(DeckCtrlDfuHeader {
        version: header[0],
        deck_ctrl_count: header[1],
        status: header[2],
    })
}

async fn print_deck_ctrl_dfu_info(raw: &RawMemory) -> Result<()> {
    let header = parse_deck_ctrl_dfu_header(&raw.read(0, 4).await?)?;

    println!("DeckCtrl DFU Memory");
    println!("  Version        : {}", header.version);
    println!("  DeckCtrl count : {}", header.deck_ctrl_count);
    println!("  In DFU mode    : {}", (header.status & DECK_CTRL_DFU_STATUS_IN_DFU_MODE) != 0);
    println!("  Can enable DFU : {}", (header.status & DECK_CTRL_DFU_STATUS_CAN_ENABLE_DFU) != 0);

    Ok(())
}

async fn print_deck_memory_info(deckmem: &DeckMemory) -> Result<()> {
    for section in deckmem.sections().iter() {
        println!("Deck Memory Section:");
        println!("  Name: {}", section.name());
        println!("  Started: {}", section.is_started().await.map_err(|e| anyhow!("Failed to get started status: {:?}", e))?);
        println!("  Supports Read: {}", section.supports_read());
        println!("  Supports Write: {}", section.supports_write());
        println!("  Supports Upgrade: {}", section.supports_upgrade());
        println!("  Upgrade Required: {}", section.upgrade_required().await.map_err(|e| anyhow!("Failed to get upgrade required status: {:?}", e))?);
        println!("  Bootloader Active: {}", section.bootloader_active().await.map_err(|e| anyhow!("Failed to get bootloader active status: {:?}", e))?);
        println!("  Can Reset to Firmware: {}", section.can_reset_to_firmware());
        println!("  Can Reset to Bootloader: {}", section.can_reset_to_bootloader());
        println!("  Required Hash: {:?}", section.required_hash());
        println!("  Required Length: {:?}", section.required_length());
    }

    Ok(())
}

pub async fn display(cf: &Crazyflie, memory: &MemoryInfo) -> Result<()> {
    let id = memory.id;
    match memory.memory_type {
        MemoryType::EEPROMConfig => {
            let eeprom = cf.memory.open_memory::<EEPROMConfigMemory>(memory).await
                .with_context(|| format!("Could not access memory ID={} as EEPROMConfig", id))?;
            print_eeprom_info(&eeprom);
        }
        MemoryType::OneWire => {
            let ow_memory = cf.memory.open_memory::<OwMemory>(memory).await
                .with_context(|| format!("Could not access memory ID={} as OneWire", id))?;
            print_ow_info(&ow_memory);
        }
        MemoryType::DeckMemory => {
            // Not in the client API yet: through crazyflie-lib (direct mode)
            let lib = crate::utils::connection::lib(cf, "Displaying deck memory")?;
            let device = lib.memory.get_memories(None).into_iter().find(|m| m.memory_id == id).cloned()
                .ok_or_else(|| anyhow!("Memory ID={} not found", id))?;
            let deck_memory = match lib.memory.open_memory::<DeckMemory>(device).await {
                Some(Ok(e)) => e,
                Some(Err(e)) => bail!("Could not access memory ID={} as DeckMemory: {}", id, e),
                None => bail!("Memory ID={} not found", id),
            };
            let shown = print_deck_memory_info(&deck_memory).await;
            lib.memory.close_memory(deck_memory).await?;
            shown?;
        }
        MemoryType::DeckCtrlDFU => {
            let raw = cf.memory.open_memory::<RawMemory>(memory).await
                .with_context(|| format!("Could not access memory ID={} as RawMemory", id))?;
            print_deck_ctrl_dfu_info(&raw).await?;
        }
        _ => bail!("Don't know how to handle memory ID={} yet, cannot display it", id),
    }

    Ok(())
}

pub async fn erase(cf: &Crazyflie, memory: &MemoryInfo) -> Result<()> {
    let id = memory.id;
    let size = match memory.memory_type {
        MemoryType::EEPROMConfig => 32,
        MemoryType::OneWire => 112,
        _ => bail!("Don't know how to handle memory ID={} yet, cannot erase it", id),
    };
    let raw = cf.memory.open_memory::<RawMemory>(memory).await
        .with_context(|| format!("Could not access memory ID={} as RawMemory", id))?;
    raw.write(0, &vec![0xFFu8; size]).await?;
    let name = if memory.memory_type == MemoryType::OneWire { "OneWire" } else { "EEPROMConfig" };
    println!("{} memory ID={} erased.", name, id);
    Ok(())
}

/// How many differing bytes `verify` lists before summarising the rest.
/// Enough to spot a pattern (a shifted image, a stuck bit) without burying
/// the summary line.
const MAX_REPORTED_MISMATCHES: usize = 16;

/// One differing byte, as reported by `mem verify`.
#[derive(Tabled)]
struct Mismatch {
    #[tabled(rename = "Offset")]
    offset: String,
    #[tabled(rename = "Expected")]
    expected: String,
    #[tabled(rename = "Actual")]
    actual: String,
}

/// Read `expected.len()` bytes back from `memory` at `offset` and compare.
///
/// Returns an error if any byte differs, so the command exits non-zero and a
/// script can act on it; the differing bytes are listed first.
pub async fn verify(
    cf: &Crazyflie,
    memory: &MemoryInfo,
    offset: usize,
    expected: &[u8],
) -> Result<()> {
    let mem_id = memory.id;

    let raw = cf.memory.open_memory::<RawMemory>(memory).await
        .with_context(|| format!("Could not access memory ID={} as raw memory", mem_id))?;

    let progress_bar = crate::utils::display::get_progressbar(expected.len(), None);
    let pb = progress_bar.clone();
    let actual = raw
        .read_with_progress(offset, expected.len(), move |bytes_read, _total| {
            pb.set_position(bytes_read as u64);
        })
        .await?;
    crate::utils::display::finish_progress(
        &progress_bar,
        format!(
            "Read back {} bytes from memory ID={} at offset 0x{:x}",
            expected.len(),
            mem_id,
            offset
        ),
    );

    let mismatches: Vec<Mismatch> = expected
        .iter()
        .zip(actual.iter())
        .enumerate()
        .filter(|(_, (want, got))| want != got)
        .map(|(i, (want, got))| Mismatch {
            offset: format!("0x{:08x}", offset + i),
            expected: format!("0x{:02X}", want),
            actual: format!("0x{:02X}", got),
        })
        .collect();

    if mismatches.is_empty() {
        println!(
            "Verify OK: {} bytes at offset 0x{:x} match",
            expected.len(),
            offset
        );
        return Ok(());
    }

    let shown = mismatches.len().min(MAX_REPORTED_MISMATCHES);
    crate::utils::display::print_table(&crate::utils::display::table(&mismatches[..shown]));
    if mismatches.len() > shown {
        println!("... and {} more", mismatches.len() - shown);
    }

    bail!(
        "Verify FAILED: {} of {} bytes differ",
        mismatches.len(),
        expected.len()
    )
}
