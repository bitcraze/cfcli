use std::process;

use crazyflie_lib::{
    subsystems::memory::{EEPROMConfigMemory, MemoryDevice, MemoryType, OwMemory, DeckMemory, RawMemory},
    Crazyflie,
};

fn print_eeprom_info(eeprom: &EEPROMConfigMemory) {
    println!("EEPROM Config:");
    println!("  Radio Channel: {}", eeprom.get_radio_channel());
    println!("  Radio Speed: {}", eeprom.get_radio_speed());
    println!("  Pitch Trim: {:.4}", eeprom.get_pitch_trim());
    println!("  Roll Trim: {:.4}", eeprom.get_roll_trim());
    println!("  Radio Address: {:02X?}", eeprom.get_radio_address());
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

async fn print_deck_memory_info(deckmem: &DeckMemory) -> Result<(), Box<dyn std::error::Error>> {
    for section in deckmem.sections().iter() {
        println!("Deck Memory Section:");
        println!("  Name: {}", section.name());
        println!("  Started: {}", section.is_started().await?);
        println!("  Supports Read: {}", section.supports_read());
        println!("  Supports Write: {}", section.supports_write());
        println!("  Supports Upgrade: {}", section.supports_upgrade());
        println!("  Upgrade Required: {}", section.upgrade_required().await?);
        println!("  Bootloader Active: {}", section.bootloader_active().await?);
        println!("  Can Reset to Firmware: {}", section.can_reset_to_firmware());
        println!("  Can Reset to Bootloader: {}", section.can_reset_to_bootloader());
        println!("  Required Hash: {:?}", section.required_hash());
        println!("  Required Length: {:?}", section.required_length());
    }

    Ok(())
}

pub async fn display(cf: &Crazyflie, memory: MemoryDevice) {
    match memory.memory_type {
        MemoryType::EEPROMConfig => {
            let eeprom = match cf
                .memory
                .open_memory::<EEPROMConfigMemory>(memory.clone())
                .await
            {
                Some(Ok(e)) => e,
                Some(Err(e)) => {
                    println!(
                        "Could not access memory ID={} as EEPROMConfig: {}",
                        memory.memory_id, e
                    );
                    process::exit(1);
                }
                None => {
                    println!("Memory ID={} not found", memory.memory_id);
                    process::exit(1);
                }
            };

            print_eeprom_info(&eeprom);
        }
        MemoryType::OneWire => {
            let ow_memory = match cf
                .memory
                .open_memory::<OwMemory>(memory.clone())
                .await
            {
                Some(Ok(o)) => o,
                Some(Err(e)) => {
                    println!(
                        "Could not access memory ID={} as OneWire: {}",
                        memory.memory_id, e
                    );
                    process::exit(1);
                }
                None => {
                    println!("Memory ID={} not found", memory.memory_id);
                    process::exit(1);
                }
            };
            print_ow_info(&ow_memory);
        }
        MemoryType::DeckMemory => {
            let deck_memory = match cf
                .memory
                .open_memory::<DeckMemory>(memory.clone())
                .await
            {
                Some(Ok(e)) => e,
                Some(Err(e)) => {
                    println!(
                        "Could not access memory ID={} as DeckMemory: {}",
                        memory.memory_id, e
                    );
                    process::exit(1);
                }
                None => {
                    println!("Memory ID={} not found", memory.memory_id);
                    process::exit(1);
                }
            };

            if let Err(e) = print_deck_memory_info(&deck_memory).await {
                println!("Error printing deck memory info: {}", e);
                process::exit(1);
            }
        }
        _ => {
            println!(
                "Don't know how to handle memory ID={} yet, cannot display it",
                memory.memory_id
            );
            process::exit(1);
        }
    }
}

pub async fn erase(cf: &Crazyflie, memory: MemoryDevice) {
    match memory.memory_type {
        MemoryType::EEPROMConfig => {
            let eeprom = match cf
                .memory
                .open_memory::<RawMemory>(memory.clone())
                .await
            {
                Some(Ok(e)) => e,
                Some(Err(e)) => {
                    println!(
                        "Could not access memory ID={} as RawMemory: {}",
                        memory.memory_id, e
                    );
                    process::exit(1);
                }
                None => {
                    println!("Memory ID={} not found", memory.memory_id);
                    process::exit(1);
                }
            };

            eeprom.write(0, &vec![0xFFu8; 32]).await.unwrap();
            println!("EEPROMConfig memory ID={} erased.", memory.memory_id);

        }
        MemoryType::OneWire => {
            let ow_memory = match cf
                .memory
                .open_memory::<RawMemory>(memory.clone())
                .await
            {
                Some(Ok(o)) => o,
                Some(Err(e)) => {
                    println!(
                        "Could not access memory ID={} as RawMemory: {}",
                        memory.memory_id, e
                    );
                    process::exit(1);
                }
                None => {
                    println!("Memory ID={} not found", memory.memory_id);
                    process::exit(1);
                }
            };
            ow_memory.write(0, &vec![0xFFu8; 112]).await.unwrap();
            println!("OneWire memory ID={} erased.", memory.memory_id);
        }
        _ => {
            println!(
                "Don't know how to handle memory ID={} yet, cannot erase it",
                memory.memory_id
            );
            process::exit(1);
        }
    }
}