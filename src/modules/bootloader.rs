use anyhow::{anyhow, bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::subsystems::memory::{DeckMemory, MemoryType};
use tokio::time::{sleep, timeout, Duration};
use crazyflie_link::{Connection, LinkContext, Packet};
use byteorder::{LittleEndian, ByteOrder};

use crate::ConfigTocCache;
use crate::utils::firmware::{Firmware, FirmwareUpgrade};
use crate::utils::display::*;

use cfloader::Bllink;

const TARGET_NRF51: u8 = 0xFE;

#[derive(Debug)]
struct BootloaderInfo {
    id: u8,
    protocol_version: u8,
    page_size: u16,
    buffer_pages: u16,
    flash_pages: u16,
    start_page: u16,
    cpuid: u16,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum BootloaderCommand {
  AllOff = 0x01,
  SysOff = 0x02,
  SysOn = 0x03,
  ResetInit = 0xFF,
  Reset = 0xF0,
}

pub fn get_hardcoded_list_of_targets() -> Vec<&'static str> {
    vec![
      "nrf51-fw",
      "bcAI:esp-fw",
      "bcLighthouse4-fw",
      "stm32-fw",
      "bcColorLedTop:col-fw",
      "bcColorLedBot:col-fw",
    ]
}

async fn scan_for_bootloader() -> Result<String> {
    let context = LinkContext::new();
    let res = context
        .scan_selected(vec![
            "radio://0/110/2M/E7E7E7E7E7",
            "radio://0/0/2M/E7E7E7E7E7",
        ])
        .await?;

    if res.is_empty() {
        Ok(String::from(""))
    } else {
        Ok(String::from(&res[0]))
    }
}

async fn get_info(link: &Connection, target: u8) -> Result<BootloaderInfo> {
    for _ in 0..5 {
        let packet: Packet = vec![0xFF, target, 0x10].into();

        link.send_packet(packet).await?;
        let packet = timeout(Duration::from_millis(100), link.recv_packet())
            .await?
            .unwrap();
        let data = packet.get_data();

        if packet.get_header() == 0xFF && data.len() >= 2 && data[0..2] == [target, 0x10] {
            return Ok(BootloaderInfo {
                id: data[0],
                protocol_version: data[1],
                page_size: LittleEndian::read_u16(&data[2..4]),
                buffer_pages: LittleEndian::read_u16(&data[4..6]),
                flash_pages: LittleEndian::read_u16(&data[6..8]),
                start_page: LittleEndian::read_u16(&data[8..10]),
                cpuid: LittleEndian::read_u16(&data[10..12]),
            });
        }
    }

    Err(anyhow!("Failed to get info"))
}

async fn reset_to_bootloader(link: &Connection) -> Result<String> {
    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF].into();
    link.send_packet(packet).await?;

    let mut new_address = Vec::new();
    loop {
        let packet = tokio::select! {
            result = link.recv_packet() => result?,
            _ = sleep(Duration::from_millis(100)) => {
              return Err(anyhow!("Disconnected: timeout waiting for response"));
            }
        };
        let data = packet.get_data();
        if data.len() > 2 && data[0..2] == [TARGET_NRF51, 0xFF] {
            new_address.push(0xb1);
            for byte in data[2..6].iter().rev() {
                // handle little-endian order
                new_address.push(*byte);
            }
            break;
        }
    }

    for _ in 0..10 {
        let packet: Packet = vec![0xFF, TARGET_NRF51, 0xF0, 0x00].into();
        link.send_packet(packet).await?;
    }
    sleep(Duration::from_millis(500)).await;

    Ok(format!(
        "radio://0/0/2M/{}?safelink=0&ackfilter=0",
        hex::encode(new_address).to_uppercase()
    ))
}

async fn reset_and_get_bootloader_address(link: &Connection) -> Result<Vec<u8>> {

    // Disable safelink so we can send "bootloader" messages to the nRF51
    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF, 0x05, 0x00].into();
    link.send_packet(packet).await?;

    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF].into();
    link.send_packet(packet).await?;

    let mut new_address = Vec::new();
    loop {
        let packet = tokio::select! {
            result = link.recv_packet() => result?,
            _ = sleep(Duration::from_millis(100)) => {
              return Err(anyhow!("Disconnected: timeout waiting for response"));
            }
        };
        let data = packet.get_data();
        if data.len() > 2 && data[0..2] == [TARGET_NRF51, 0xFF] {
            new_address.push(0xb1);
            for byte in data[2..6].iter().rev() {
                // handle little-endian order
                new_address.push(*byte);
            }
            break;
        }
    }

    for _ in 0..10 {
        let packet: Packet = vec![0xFF, TARGET_NRF51, 0xF0, 0x00].into();
        link.send_packet(packet).await?;
    }
    sleep(Duration::from_millis(500)).await;

    Ok(new_address)
}

async fn start_bootloader(context: &LinkContext, cold: bool, uri: &str) -> Result<Connection> {
    let uri: String = if cold {
        scan_for_bootloader().await
    } else {
        let link = context.open_link(&format!("{}?safelink=0", uri)).await?;
        let uri = reset_to_bootloader(&link).await;
        link.close().await;
        sleep(Duration::from_millis(500)).await;
        uri  
    }?;

    let link = context.open_link(&uri).await?;
    Ok(link)
}

async fn restart_and_get_bllink(context: &LinkContext, uri: &str, cold: bool) -> Result<Bllink> {
    let address: Option<[u8; 5]> = match cold {
        false => {
              let link = context.open_link(&format!("{}?safelink=0", uri)).await?;
              let new_address = reset_and_get_bootloader_address(&link).await?;
              link.close().await;
              let arr: [u8; 5] = new_address.try_into().map_err(|_| anyhow!("Address must be exactly 5 bytes"))?;
              Some(arr)
        }
        true => {
            None
        }
    };

    let bllink = Bllink::new(address.as_ref()).await?;

    Ok(bllink)
}

async fn send_command(link: &Connection, cmd: BootloaderCommand, data: Option<&[u8]>) -> Result<()> {

    let mut command = vec![0xFF, TARGET_NRF51, cmd as u8];
    if let Some(d) = data {
        command.extend_from_slice(d);
    }

    let packet: Packet = command.into();
    link.send_packet(packet).await?;

    Ok(())
}

pub async fn print_bootloader_info(link_context: &crazyflie_link::LinkContext, cold: bool, uri: &str) -> Result<()> {

  let link = start_bootloader(link_context, cold, uri).await?;
  let info = get_info(&link, TARGET_NRF51).await?;

  println!("Bootloader Info:");
  println!("ID: 0x{:02X}", info.id);
  println!("Protocol Version: {}", info.protocol_version);
  println!("Page Size: {} bytes", info.page_size);
  println!("Buffer Pages: {}", info.buffer_pages);
  println!("Flash Pages: {}", info.flash_pages);
  println!("Start Page: {}", info.start_page);
  println!("CPU ID: 0x{:04X}", info.cpuid);

  link.close().await;

  Ok(())
}

pub async fn reboot(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<()> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::ResetInit, None).await?;
  send_command(&link, BootloaderCommand::Reset, Some(&[0x01])).await?; // Reset to firmware
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

pub async fn power_off(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<()> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::AllOff, None).await?;
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

pub async fn sysoff(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<()> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::SysOff, None).await?;
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

pub async fn syson(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<()> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::SysOn, None).await?;
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

async fn get_flashable_firmware(cf: &Crazyflie, firmwares: &[Firmware]) -> Result<Vec<Firmware>> {
      let mut flashable_firmares = Vec::new(); 
      let memories = cf.memory.get_memories(Some(MemoryType::DeckMemory));
      if !memories.is_empty() {
        let deck_memory = match cf.memory.open_memory::<DeckMemory>(memories[0].clone()).await {
          Some(Ok(deck)) => deck,
          Some(Err(e)) => {
            return Err(anyhow!("Error: {:?}", e));
          }
          None => {
            return Err(anyhow!("DeckMemory not found"));
          }
        };

        for firmware in firmwares {
          if firmware.target != "stm32" && firmware.target != "nrf51" {
            let section = deck_memory.sections().iter().find(|s| s.name() == firmware.target);
            if let Some(_section) = section {
              flashable_firmares.push(firmware.clone());
            }
          } 
        }

        cf.memory.close_memory(deck_memory).await?;

      }

      Ok(flashable_firmares)
}

async fn is_aideck_attached(cf: &Crazyflie) -> Result<bool> {
    let memories = cf.memory.get_memories(Some(MemoryType::DeckMemory));
    if !memories.is_empty() {
      let deck_memory = match cf.memory.open_memory::<DeckMemory>(memories[0].clone()).await {
        Some(Ok(deck)) => deck,
        Some(Err(e)) => {
          return Err(anyhow!("Error: {:?}", e));
        }
        None => {
          bail!("DeckMemory not found");
        }
      };

      let section = deck_memory.sections().iter().find(|s| s.name() == "bcAI:esp-fw");
      if let Some(_section) = section {
        return Ok(true);
      } 
    }

    Ok(false)
}

pub async fn flash(link_context: &crazyflie_link::LinkContext, uri: &str, toc_cache: ConfigTocCache, firmware_upgrade: FirmwareUpgrade, cold: bool) -> Result<()> {

  let firmware_for_bootloader = firmware_upgrade.get_firmware_for_bootloader();
  let firmware_for_decks = firmware_upgrade.get_firmware_for_decks();

  if !firmware_for_bootloader.is_empty() {
      let bllink = restart_and_get_bllink(link_context, uri, cold).await?;
      let mut cfloader = cfloader::CFLoader::new(bllink).await?;
      for firmware in firmware_for_bootloader {
        if firmware.target == "stm32" && firmware.file_type == "fw" {
          let stm32_info = cfloader.stm32_info();
          let start_address = stm32_info.flash_start() as u32 * stm32_info.page_size() as u32;
          let progress_bar = get_progressbar(firmware.data.len(), Some(firmware.target.as_str()));   
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          cfloader.flash_stm32_with_progress(start_address, &firmware.data, Some(progress_callback)).await?;
          progress_bar.finish_with_message("STM32F405 flashed successfully!");
        }
        if firmware.target == "nrf51" && firmware.file_type == "fw" {
          let nrf51_info = cfloader.nrf51_info();
          let start_address = nrf51_info.flash_start() as u32 * nrf51_info.page_size() as u32;
          let progress_bar = get_progressbar(firmware.data.len(), Some(firmware.target.as_str()));   
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          cfloader.flash_nrf51_with_progress(start_address, &firmware.data, Some(progress_callback)).await?;
          progress_bar.finish_with_message("nRF51822 flashed successfully!");
        }
    }
    cfloader.reset_to_firmware().await?;

    if !firmware_for_decks.is_empty() {
        println!("Wait for Crazyflie to restart...");
        // Wait for Crazyflie to start up when going from bootloader->firmware
        // The long wait is due to AI-deck startup delay
        sleep(Duration::from_millis(7000)).await;
    }
  }

  if !firmware_for_decks.is_empty() {
    // We need to disconnect the Crazyflie and reconnect again after each deck flash. Inbetween
    // reboots we also need to wait. So connect once and filter out all the decks we cannot
    // flash.

    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
          &link_context,
          uri,
          toc_cache.clone()
    ).await?;

    let firmware_for_decks = get_flashable_firmware(&cf, &firmware_for_decks).await?;
    let delay = if is_aideck_attached(&cf).await? {
      7000
    } else {
      3000
    };
    let mut flash_count_left = firmware_for_decks.len();
    cf.disconnect().await;

    for firmware in &firmware_for_decks {
      let cf = crazyflie_lib::Crazyflie::connect_from_uri(
          &link_context,
          uri,
          toc_cache.clone()
      ).await?;

      let memories = cf.memory.get_memories(Some(MemoryType::DeckMemory));
      if !memories.is_empty() {
        let deck_memory = match cf.memory.open_memory::<DeckMemory>(memories[0].clone()).await {
          Some(Ok(deck)) => deck,
          Some(Err(e)) => {
            return Err(anyhow!("Error: {:?}", e));
          }
          None => {
            return Err(anyhow!("DeckMemory not found"));
          }
        };
        
        let section = deck_memory.sections().iter().find(|s| s.name() == firmware.target);

        if let Some(section) = section {

          let bootloader_active = section.bootloader_active().await?;
          if !bootloader_active {
            section.reset_to_bootloader().await?;
            sleep(Duration::from_millis(10)).await;

            let bootloader_active = section.bootloader_active().await?;
            if !bootloader_active {
              bail!("Failed to activate bootloader for deck section");
            }
          }

          let progress_bar = get_progressbar(firmware.data.len(), Some(section.name()));   
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          section.write_with_progress(0, &firmware.data, progress_callback).await?;
          progress_bar.finish_with_message("Deck firmware flashed successfully!");
        }

        cf.disconnect().await;

      }

      flash_count_left = flash_count_left - 1;

      reboot(&link_context, uri).await?;

      if flash_count_left > 0 {
          println!("Restarting Crazyflie...");
          sleep(Duration::from_millis(delay)).await;
      }
    }
  }

  Ok(())
}