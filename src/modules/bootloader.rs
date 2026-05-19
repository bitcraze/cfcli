use anyhow::{anyhow, bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::subsystems::memory::{DeckMemory, MemoryType, RawMemory};
use tokio::time::{sleep, timeout, Duration};
use crazyflie_link::{Connection, LinkContext, Packet};
use byteorder::{LittleEndian, ByteOrder};

use crate::ConfigTocCache;
use crate::error::CliError;
use crate::modules::memory::{
    read_deck_ctrl_dfu_header,
    DECK_CTRL_DFU_STATUS_CAN_ENABLE_DFU,
    DECK_CTRL_DFU_STATUS_IN_DFU_MODE,
};
use crate::utils::firmware::{Firmware, FirmwareUpgrade, FlashStartOverride};
use crate::utils::display::*;

const DECK_CTRL_DFU_FLASH_OFFSET: usize = 0x10000;
const DECK_CTRL_DFU_CFG_DEFAULT_OFFSET: usize = 0x17800;
const DECK_CTRL_DFU_PAGE_SIZE: usize = 1024;
const DECK_CTRL_DFU_CMD_OFFSET: usize = 0x03;
const DECK_CTRL_DFU_CMD_ENTER_DFU: u8 = 0x01;
const DECK_CTRL_DFU_CMD_ENTER_FIRMWARE: u8 = 0x02;
const DECK_CTRL_DFU_RESET_DELAY_MS: u64 = 3000;

use cfloader::Bllink;

const TARGET_STM32: u8 = 0xFF;
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
  ResetInit = 0xFF,
  Reset = 0xF0,
}

pub fn get_hardcoded_list_of_targets() -> Vec<&'static str> {
    vec![
      "nrf51-fw",
      "bcAI:esp-fw",
      "bcCam:qcc",
      "bcLighthouse4-fw",
      "stm32-fw",
      "bcColorLedTop:col-fw",
      "bcColorLedBot:col-fw",
      "deckctrl-fw",
      "deckctrl-cfg",
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
        let separator = if uri.contains('?') { "&" } else { "?" };
        let link = context.open_link(&format!("{}{}safelink=0", uri, separator)).await?;
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
              let separator = if uri.contains('?') { "&" } else { "?" };
              let link = context.open_link(&format!("{}{}safelink=0", uri, separator)).await?;
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

fn print_target_info(name: &str, info: &BootloaderInfo) {
  let flash_size = info.flash_pages as u32 * info.page_size as u32;
  let available_flash = (info.flash_pages - info.start_page) as u32 * info.page_size as u32;

  println!("{} (ID: 0x{:02X}):", name, info.id);
  println!("  Protocol Version: {}", info.protocol_version);
  println!("  Page Size: {} bytes", info.page_size);
  println!("  Buffer Pages: {}", info.buffer_pages);
  println!("  Flash Pages: {}", info.flash_pages);
  println!("  Flash Size: {} bytes ({} KB)", flash_size, flash_size / 1024);
  println!("  Start Page: {}", info.start_page);
  println!("  Available Flash: {} bytes ({} KB)", available_flash, available_flash / 1024);
  println!("  CPU ID: 0x{:04X}", info.cpuid);
}

pub async fn print_bootloader_info(link_context: &crazyflie_link::LinkContext, cold: bool, uri: &str) -> Result<()> {

  let link = start_bootloader(link_context, cold, uri).await?;

  println!("Bootloader Info:");
  println!();

  let stm32_info = get_info(&link, TARGET_STM32).await?;
  print_target_info("STM32", &stm32_info);

  println!();

  let nrf51_info = get_info(&link, TARGET_NRF51).await?;
  print_target_info("nRF51", &nrf51_info);

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

async fn open_deck_ctrl_dfu_raw(cf: &Crazyflie) -> Result<RawMemory> {
    let memories = cf.memory.get_memories(Some(MemoryType::DeckCtrlDFU));
    if memories.is_empty() {
        bail!("DeckCtrlDFU memory not present, cannot flash DeckCtrl");
    }
    if memories.len() > 1 {
        bail!("Multiple DeckCtrlDFU memories found ({}), cannot flash DeckCtrl", memories.len());
    }
    match cf.memory.open_memory::<RawMemory>(memories[0].clone()).await {
        Some(Ok(m)) => Ok(m),
        Some(Err(e)) => bail!("Could not access DeckCtrlDFU memory: {}", e),
        None => bail!("DeckCtrlDFU memory not found"),
    }
}

async fn flash_deck_ctrl(
    link_context: &LinkContext,
    uri: &str,
    toc_cache: ConfigTocCache,
    firmwares: &[Firmware],
) -> Result<()> {
    if firmwares.is_empty() {
        return Ok(());
    }

    let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache.clone()).await?;
    let raw = open_deck_ctrl_dfu_raw(&cf).await?;
    let header = read_deck_ctrl_dfu_header(&raw).await?;

    if header.version != 1 {
        bail!("Unsupported DeckCtrlDFU version: {}", header.version);
    }

    let already_in_dfu = (header.status & DECK_CTRL_DFU_STATUS_IN_DFU_MODE) != 0;

    if !already_in_dfu {
        if header.deck_ctrl_count > 1 {
            bail!(
                "Cannot enter DFU: expected at most one DeckCtrl deck attached, found {}",
                header.deck_ctrl_count
            );
        }
        if (header.status & DECK_CTRL_DFU_STATUS_CAN_ENABLE_DFU) == 0 {
            bail!("Cannot enter DFU: STATUS_CAN_ENABLE_DFU is not set");
        }

        raw.write(DECK_CTRL_DFU_CMD_OFFSET, &[DECK_CTRL_DFU_CMD_ENTER_DFU]).await?;
        cf.disconnect().await;
        sleep(Duration::from_millis(DECK_CTRL_DFU_RESET_DELAY_MS)).await;
    } else {
        cf.disconnect().await;
    }

    // Reconnect — DFU entry power-cycles the Crazyflie and memory IDs may be
    // re-enumerated, so we re-look up the memory after reconnecting.
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache.clone()).await?;
    let raw = open_deck_ctrl_dfu_raw(&cf).await?;
    let header = read_deck_ctrl_dfu_header(&raw).await?;
    if (header.status & DECK_CTRL_DFU_STATUS_IN_DFU_MODE) == 0 {
        bail!("DeckCtrl did not enter DFU mode");
    }

    // Flash firmware first, then config
    let mut sorted: Vec<&Firmware> = firmwares.iter().collect();
    sorted.sort_by_key(|f| match f.file_type.as_str() {
        "fw" => 0,
        "cfg" => 1,
        _ => 2,
    });

    for fw in sorted {
        let address: usize = match &fw.start_override {
            Some(FlashStartOverride::Address(addr)) => *addr as usize,
            Some(FlashStartOverride::Page(page)) => {
                DECK_CTRL_DFU_FLASH_OFFSET + (*page as usize) * DECK_CTRL_DFU_PAGE_SIZE
            }
            None => match fw.file_type.as_str() {
                "fw" => DECK_CTRL_DFU_FLASH_OFFSET,
                "cfg" => DECK_CTRL_DFU_CFG_DEFAULT_OFFSET,
                other => bail!("Unknown deckctrl file type '{}', cannot pick default address", other),
            },
        };

        let label = format!("deckctrl-{}", fw.file_type);
        let progress_bar = get_progressbar(fw.data.len(), Some(&label));
        let pb = progress_bar.clone();
        let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
        };
        raw.write_with_progress(address, &fw.data, progress_callback).await?;
        finish_progress(&progress_bar, format!("DeckCtrl {} flashed successfully!", fw.file_type));
    }

    raw.write(DECK_CTRL_DFU_CMD_OFFSET, &[DECK_CTRL_DFU_CMD_ENTER_FIRMWARE]).await?;
    cf.disconnect().await;
    sleep(Duration::from_millis(DECK_CTRL_DFU_RESET_DELAY_MS)).await;

    Ok(())
}

pub async fn flash(link_context: &crazyflie_link::LinkContext, uri: &str, toc_cache: ConfigTocCache, firmware_upgrade: FirmwareUpgrade, cold: bool) -> Result<()> {

  let firmware_for_bootloader = firmware_upgrade.get_firmware_for_bootloader();
  let firmware_for_deckctrl = firmware_upgrade.get_firmware_for_deckctrl();
  let firmware_for_decks = firmware_upgrade.get_firmware_for_decks();

  if !firmware_for_bootloader.is_empty() {
      // stm32-fw / nrf51-fw flashing requires the radio bootloader handshake;
      // USB doesn't expose that path. Decks (handled below) work over USB.
      // In --cold mode the URI is ignored (we scan for the rescue bootloader),
      // so a usb:// URI is fine there.
      if uri.starts_with("usb://") && !cold {
          bail!(CliError::InvalidValue(
              "stm32-fw / nrf51-fw flashing requires a radio URI; USB only supports deck/deck-ctrl targets".to_string()
          ));
      }
      let bllink = restart_and_get_bllink(link_context, uri, cold).await?;
      let mut cfloader = cfloader::CFLoader::new(bllink).await?;
      for firmware in firmware_for_bootloader {
        if firmware.target == "stm32" && firmware.file_type == "fw" {
          let stm32_info = cfloader.stm32_info();
          let start_address = match &firmware.start_override {
            Some(FlashStartOverride::Address(addr)) => *addr,
            Some(FlashStartOverride::Page(page)) => *page as u32 * stm32_info.page_size() as u32,
            None => stm32_info.flash_start() as u32 * stm32_info.page_size() as u32,
          };
          let progress_bar = get_progressbar(firmware.data.len(), Some(firmware.target.as_str()));
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          cfloader.flash_stm32_with_progress(start_address, &firmware.data, Some(progress_callback)).await?;
          finish_progress(&progress_bar, "STM32F405 flashed successfully!");
        }
        if firmware.target == "nrf51" && firmware.file_type == "fw" {
          let nrf51_info = cfloader.nrf51_info();
          let start_address = match &firmware.start_override {
            Some(FlashStartOverride::Address(addr)) => *addr,
            Some(FlashStartOverride::Page(page)) => *page as u32 * nrf51_info.page_size() as u32,
            None => nrf51_info.flash_start() as u32 * nrf51_info.page_size() as u32,
          };
          let progress_bar = get_progressbar(firmware.data.len(), Some(firmware.target.as_str()));
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          cfloader.flash_nrf51_with_progress(start_address, &firmware.data, Some(progress_callback)).await?;
          finish_progress(&progress_bar, "nRF51822 flashed successfully!");
        }
    }
    cfloader.reset_to_firmware().await?;

    if !firmware_for_decks.is_empty() || !firmware_for_deckctrl.is_empty() {
        println!("Wait for Crazyflie to restart...");
        // Wait for Crazyflie to start up when going from bootloader->firmware
        // The long wait is due to AI-deck startup delay
        sleep(Duration::from_millis(7000)).await;
    }
  }

  if !firmware_for_deckctrl.is_empty() {
    flash_deck_ctrl(link_context, uri, toc_cache.clone(), &firmware_for_deckctrl).await?;
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
        // Write the new-fw-size to the deck's command region before the
        // bulk write so the deck-side driver can erase the right region
        // up front. The DeckMemorySection abstraction doesn't expose the
        // command/info addresses, so do this via RawMemory: parse the
        // info section to locate the protocol section index by name,
        // then write 4 LE bytes at 0x1000 + idx*0x20.
        let raw = match cf.memory.open_memory::<RawMemory>(memories[0].clone()).await {
          Some(Ok(r)) => r,
          Some(Err(e)) => return Err(anyhow!("Open DeckMemory as raw: {:?}", e)),
          None => return Err(anyhow!("DeckMemory not found")),
        };
        let mut proto_idx: Option<usize> = None;
        for i in 0..8usize {
          let data = raw.read(1 + i * 0x20, 0x20).await?;
          if data.len() < 0x20 { break; }
          if (data[0] & 0x01) == 0 { continue; }
          let name: String = data[14..32].iter()
              .take_while(|&&b| b != 0)
              .map(|&b| b as char)
              .collect();
          if name == firmware.target {
            proto_idx = Some(i);
            break;
          }
        }
        if let Some(idx) = proto_idx {
          let cmd_addr = 0x1000 + idx * 0x20;
          let size_bytes = (firmware.data.len() as u32).to_le_bytes();
          raw.write(cmd_addr, &size_bytes).await?;
        }
        cf.memory.close_memory(raw).await?;

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

            // The deck may take a couple of seconds to complete the
            // ROM-bootloader handshake before flipping the active bit,
            // so poll up to 5 s instead of a single check.
            let mut active = false;
            for _ in 0..50 {
              sleep(Duration::from_millis(100)).await;
              if section.bootloader_active().await? {
                active = true;
                break;
              }
            }
            if !active {
              bail!("Failed to activate bootloader for deck section");
            }
          }

          let progress_bar = get_progressbar(firmware.data.len(), Some(section.name()));   
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          section.write_with_progress(0, &firmware.data, progress_callback).await?;
          finish_progress(&progress_bar, "Deck firmware flashed successfully!");
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
