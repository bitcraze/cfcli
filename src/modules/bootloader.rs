use anyhow::{anyhow, bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::subsystems::memory::{DeckMemory, MemoryDevice, MemoryType, OwMemory, RawMemory};
use tokio::time::{sleep, Duration};
use crazyflie_lib::crazyflie_link::{Connection, LinkContext, Packet};
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
use crate::utils::radio::RadioUri;

const DECK_CTRL_DFU_FLASH_OFFSET: usize = 0x10000;
const DECK_CTRL_DFU_CFG_DEFAULT_OFFSET: usize = 0x17800;
const DECK_CTRL_DFU_PAGE_SIZE: usize = 1024;
const DECK_CTRL_DFU_CMD_OFFSET: usize = 0x03;
const DECK_CTRL_DFU_CMD_ENTER_DFU: u8 = 0x01;
const DECK_CTRL_DFU_CMD_ENTER_FIRMWARE: u8 = 0x02;
const DECK_CTRL_DFU_RESET_DELAY_MS: u64 = 3000;

// AI-deck identification, matching what the nRF51 checks at boot
const AIDECK_VID: u8 = 0xBC;
const AIDECK_PID: u8 = 0x12;
const AIDECK_BOARD_NAME: &str = "bcAI";

// Time between deck flashes for the Crazyflie to restart. The nRF51 delays
// boot by an additional 5 s when an AI-deck is attached.
const DECK_REBOOT_DELAY_MS: u64 = 3000;
const AIDECK_BOOT_DELAY_MS: u64 = 5000;

use cfloader::Bllink;

const TARGET_STM32: u8 = 0xFF;
const TARGET_NRF51: u8 = 0xFE;

// The STM32 ROM bootloader on USB, and how long it gets to show up there
// after the nRF51 has restarted the STM32 into it
const STM32_DFU_VID: u16 = 0x0483;
const STM32_DFU_PID: u16 = 0xDF11;
const STM32_DFU_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(5);

// How long a bootloader gets to answer GET_INFO, and how often it is asked
const GET_INFO_TIMEOUT: Duration = Duration::from_secs(2);
const GET_INFO_RESEND: Duration = Duration::from_millis(100);

// Attempts at restarting a Crazyflie from its bootloader into its firmware,
// and the packets sent to see whether the bootloader is gone after each
const RESTART_ATTEMPTS: usize = 3;
const BOOTLOADER_PROBES: usize = 3;

#[derive(Debug)]
struct BootloaderInfo {
    id: u8,
    protocol_version: u8,
    page_size: u16,
    buffer_pages: u16,
    flash_pages: u16,
    start_page: u16,
    cpuid: Vec<u8>,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum BootloaderCommand {
  ResetInit = 0xFF,
  Reset = 0xF0,
  Stm32Dfu = 0x07,
}

pub fn get_hardcoded_list_of_targets() -> Vec<&'static str> {
    vec![
      "nrf51-fw",
      "nrf51-bootloader+softdevice",
      "bcAI:esp-fw",
      "bcAI:gap8-fw",
      "bcCam:qcc",
      "bcLighthouse4-fw",
      "stm32-fw",
      "bcColorLedTop:col-fw",
      "bcColorLedBot:col-fw",
      "bcFlow3:rp2350-fw",
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

/// Send `request` until a packet comes back that `answer` makes something
/// of, and return that, or `None` when nothing did within `within`.
///
/// The request or its answer can be lost, so the request is sent again every
/// `resend` until it is answered. While waiting, the link keeps delivering
/// other packets: with safelink off, every ACK without a payload arrives as
/// a 0xFF packet, about one per millisecond. So the deadline and the resend
/// are checked on every packet, never after a quiet period, which would
/// never come.
async fn request<T>(
    link: &Connection,
    request: Packet,
    resend: Duration,
    within: Duration,
    answer: impl Fn(&Packet) -> Option<T>,
) -> Result<Option<T>> {
    let deadline = tokio::time::Instant::now() + within;
    let mut last_sent: Option<tokio::time::Instant> = None;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        if last_sent.is_none_or(|sent| now - sent >= resend) {
            link.send_packet(request.clone()).await?;
            last_sent = Some(now);
        }

        let wait_until = (last_sent.unwrap() + resend).min(deadline);
        let Ok(packet) = tokio::time::timeout_at(wait_until, link.recv_packet()).await else {
            continue;
        };
        if let Some(answer) = answer(&packet?) {
            return Ok(Some(answer));
        }
    }
}

/// Ask a bootloader for its info (GET_INFO), again every 100 ms for up to
/// 2 s.
async fn get_info(link: &Connection, target: u8) -> Result<BootloaderInfo> {
    let get_info: Packet = vec![0xFF, target, 0x10].into();
    let info = request(link, get_info, GET_INFO_RESEND, GET_INFO_TIMEOUT, |packet| {
        let data = packet.get_data();

        // [target, GET_INFO, page size (2), buffer pages (2), flash pages (2),
        // start page (2), CPU ID (12), protocol version]. Bootloaders from
        // before the protocol version stop after the CPU ID.
        (packet.get_header() == 0xFF && data.len() >= 22 && data[0..2] == [target, 0x10]).then(|| BootloaderInfo {
            id: data[0],
            protocol_version: data.get(22).copied().unwrap_or(0),
            page_size: LittleEndian::read_u16(&data[2..4]),
            buffer_pages: LittleEndian::read_u16(&data[4..6]),
            flash_pages: LittleEndian::read_u16(&data[6..8]),
            start_page: LittleEndian::read_u16(&data[8..10]),
            cpuid: data[10..22].to_vec(),
        })
    })
    .await?;

    let name = if target == TARGET_STM32 { "STM32" } else { "nRF51" };
    info.ok_or_else(|| {
        anyhow!(CliError::Timeout(format!(
            "the {} bootloader did not answer GET_INFO within {} s",
            name,
            GET_INFO_TIMEOUT.as_secs()
        )))
    })
}

/// Ask the nRF51 firmware for the address its bootloader will use
/// (RESET_INIT), again every 200 ms for up to 2 s.
async fn request_bootloader_address(link: &Connection) -> Result<Vec<u8>> {
    let reset_init: Packet = vec![0xFF, TARGET_NRF51, 0xFF].into();
    let address = request(link, reset_init, Duration::from_millis(200), Duration::from_secs(2), |packet| {
        let data = packet.get_data();
        (data.len() > 5 && data[0..2] == [TARGET_NRF51, 0xFF]).then(|| {
            // 0xB1 followed by the four address bytes, sent little endian
            let mut new_address = vec![0xb1];
            new_address.extend(data[2..6].iter().rev());
            new_address
        })
    })
    .await?;

    address.ok_or_else(|| anyhow!("Disconnected: the nRF51 did not answer RESET_INIT within 2 s"))
}

/// Restart the nRF51 into its bootloader and return the URI to reach the
/// bootloader at. A Crazyflie restarted this way only answers there, on
/// channel 0, until it is restarted again.
async fn reset_to_bootloader(link: &Connection, radio: usize) -> Result<String> {
    let new_address = request_bootloader_address(link).await?;

    for _ in 0..10 {
        let packet: Packet = vec![0xFF, TARGET_NRF51, 0xF0, 0x00].into();
        link.send_packet(packet).await?;
    }
    sleep(Duration::from_millis(500)).await;

    Ok(format!(
        "radio://{}/0/2M/{}?safelink=0&ackfilter=0",
        radio,
        hex::encode(new_address).to_uppercase()
    ))
}

async fn reset_and_get_bootloader_address(link: &Connection) -> Result<Vec<u8>> {

    // Disable safelink so we can send "bootloader" messages to the nRF51
    let packet: Packet = vec![0xFF, TARGET_NRF51, 0xFF, 0x05, 0x00].into();
    link.send_packet(packet).await?;

    let new_address = request_bootloader_address(link).await?;

    for _ in 0..10 {
        let packet: Packet = vec![0xFF, TARGET_NRF51, 0xF0, 0x00].into();
        link.send_packet(packet).await?;
    }
    sleep(Duration::from_millis(500)).await;

    Ok(new_address)
}

/// Restart a Crazyflie that runs its firmware into its bootloader, and
/// return the URI to reach the bootloader at. The bootloader is reached
/// with the same Crazyradio as the firmware, the first one for usb://.
async fn enter_bootloader(context: &LinkContext, uri: &str) -> Result<String> {
    let radio = RadioUri::parse(uri).ok().flatten().and_then(|r| r.radio).unwrap_or(0);
    let separator = if uri.contains('?') { "&" } else { "?" };
    let link = context.open_link(&format!("{}{}safelink=0", uri, separator)).await?;
    let bootloader_uri = reset_to_bootloader(&link, radio).await;
    link.close().await;
    sleep(Duration::from_millis(500)).await;
    bootloader_uri
}

/// Whether a bootloader answers at `bootloader_uri`. A scan that fails
/// counts as answering: it doesn't show that the bootloader is gone.
async fn bootloader_answers(context: &LinkContext, bootloader_uri: &str) -> bool {
    for _ in 0..BOOTLOADER_PROBES {
        match context.scan_selected(vec![bootloader_uri]).await {
            Ok(found) if found.is_empty() => continue,
            _ => return true,
        }
    }
    false
}

/// Restart a Crazyflie from its bootloader back into its firmware. Returns
/// whether the bootloader is gone.
async fn restart_into_firmware(context: &LinkContext, bootloader_uri: &str) -> bool {
    for _ in 0..RESTART_ATTEMPTS {
        // Whether this worked shows from the bootloader being gone.
        if let Ok(link) = context.open_link(bootloader_uri).await {
            let _ = send_command(&link, BootloaderCommand::ResetInit, None).await;
            let _ = send_command(&link, BootloaderCommand::Reset, Some(&[0x01])).await; // Reset to firmware
            sleep(Duration::from_millis(500)).await;
            link.close().await;
        }
        if !bootloader_answers(context, bootloader_uri).await {
            return true;
        }
    }
    false
}

/// The bootloader info of a Crazyflie that runs its firmware: (STM32,
/// nRF51). The Crazyflie is restarted into its bootloader to read it, and
/// back into its firmware afterwards, also when reading fails: one left in
/// its bootloader no longer answers at its URI, and the bootloader stays
/// until the power button restarts it.
async fn read_info_warm(context: &LinkContext, uri: &str) -> Result<(BootloaderInfo, BootloaderInfo)> {
    let bootloader_uri = enter_bootloader(context, uri).await?;
    let info = match context.open_link(&bootloader_uri).await {
        Ok(link) => {
            let info = read_info(&link).await;
            link.close().await;
            info
        }
        Err(e) => Err(e.into()),
    };
    if restart_into_firmware(context, &bootloader_uri).await {
        return info;
    }

    let address = bootloader_uri.split('?').next().unwrap_or_default();
    let stuck = format!(
        "the Crazyflie is still in its bootloader, at {}. Restart it with its power button",
        address
    );
    bail!(CliError::Connection(match info {
        Ok(_) => stuck,
        Err(read) => format!("{:#}, and {}", read, stuck),
    }))
}

/// The bootloader info of both MCUs: (STM32, nRF51).
async fn read_info(link: &Connection) -> Result<(BootloaderInfo, BootloaderInfo)> {
    let stm32 = get_info(link, TARGET_STM32).await?;
    let nrf51 = get_info(link, TARGET_NRF51).await?;
    Ok((stm32, nrf51))
}

/// Restart a Crazyflie that runs its firmware into its bootloader, and return
/// the address the bootloader listens on (on channel 0), for cfloader.
pub async fn enter_bootloader_for_cfloader(context: &LinkContext, uri: &str) -> Result<[u8; 5]> {
    let separator = if uri.contains('?') { "&" } else { "?" };
    let link = context.open_link(&format!("{}{}safelink=0", uri, separator)).await?;
    let new_address = reset_and_get_bootloader_address(&link).await;
    link.close().await;
    new_address?.try_into().map_err(|_| anyhow!("Address must be exactly 5 bytes"))
}

/// The URI of a bootloader at `address`, reached with Crazyradio `radio`.
pub fn bootloader_uri(radio: usize, address: &[u8; 5]) -> String {
    format!("radio://{}/0/2M/{}?safelink=0&ackfilter=0", radio, hex::encode_upper(address))
}

/// Make sure a Crazyflie that was sent from its bootloader into its
/// firmware has left the bootloader at `bootloader_uri`, and restart it again
/// if not. Returns whether the bootloader is gone.
pub async fn left_bootloader(context: &LinkContext, bootloader_uri: &str) -> bool {
    !bootloader_answers(context, bootloader_uri).await || restart_into_firmware(context, bootloader_uri).await
}

async fn restart_and_get_bllink(context: &LinkContext, uri: &str, cold: bool) -> Result<Bllink> {
    let address: Option<[u8; 5]> = match cold {
        false => Some(enter_bootloader_for_cfloader(context, uri).await?),
        true => {
            None
        }
    };

    let bllink = Bllink::new(address.as_ref()).await?;

    Ok(bllink)
}

/// Where an STM32 or nRF51 image is flashed: the address or page given with
/// --bin, or else the start of the firmware area.
pub(crate) fn start_address(firmware: &Firmware, info: &cfloader::packets::InfoPacket) -> u32 {
    match &firmware.start_override {
        Some(FlashStartOverride::Address(addr)) => *addr,
        Some(FlashStartOverride::Page(page)) => *page as u32 * info.page_size() as u32,
        None => info.flash_start() as u32 * info.page_size() as u32,
    }
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
  println!("  CPU ID: {}", hex::encode_upper(&info.cpuid));
}

/// Print the bootloader info of a Crazyflie. With `cold` the Crazyflie was
/// started into its bootloader with its power button and stays there,
/// otherwise it is restarted into its bootloader and back into its firmware.
pub async fn print_bootloader_info(link_context: &LinkContext, cold: bool, uri: &str) -> Result<()> {

  let (stm32_info, nrf51_info) = if cold {
    let link = link_context.open_link(&scan_for_bootloader().await?).await?;
    let info = read_info(&link).await;
    link.close().await;
    info?
  } else {
    read_info_warm(link_context, uri).await?
  };

  println!("Bootloader Info:");
  println!();
  print_target_info("STM32", &stm32_info);
  println!();
  print_target_info("nRF51", &nrf51_info);

  Ok(())
}

/// The bootloader protocol versions of a Crazyflie that runs its firmware:
/// (STM32, nRF51). The Crazyflie is restarted into its bootloader to read
/// them, and back into its firmware afterwards.
pub async fn bootloader_versions(link_context: &LinkContext, uri: &str) -> Result<(u8, u8)> {
  let (stm32, nrf51) = read_info_warm(link_context, uri).await?;
  Ok((stm32.protocol_version, nrf51.protocol_version))
}

/// Restart the STM32 into its ROM bootloader, so it can be flashed over USB DFU
///
/// The nRF51 power-cycles the STM32 with BOOT0 high, as a long press on the
/// power button from off does. The command is handled by the nRF51 itself,
/// which only receives it over the radio: the STM32 firmware does not pass it
/// on from USB. Returns whether the STM32 then showed up on USB in DFU mode.
pub async fn stm32_dfu(link_context: &LinkContext, uri: &str) -> Result<bool> {
  if uri.starts_with("usb://") {
    bail!("Restarting the STM32 into DFU needs a radio URI, the nRF51 does not get this command over USB");
  }

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::Stm32Dfu, None).await?;

  let deadline = tokio::time::Instant::now() + STM32_DFU_ENUMERATION_TIMEOUT;
  while tokio::time::Instant::now() < deadline {
    sleep(Duration::from_millis(200)).await;
    if stm32_dfu_present()? {
      return Ok(true);
    }
  }

  Ok(false)
}

fn stm32_dfu_present() -> Result<bool> {
  for device in rusb::devices()?.iter() {
    let desc = device.device_descriptor()?;
    if desc.vendor_id() == STM32_DFU_VID && desc.product_id() == STM32_DFU_PID {
      return Ok(true);
    }
  }
  Ok(false)
}

pub async fn reboot(link_context: &LinkContext, uri: &str,) -> Result<()> {

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

/// Detect an AI-deck the same way the nRF51 does when deciding to delay boot:
/// a valid 1-wire deck EEPROM with the Bitcraze VID, AI-deck PID and board name.
async fn is_aideck_attached(cf: &Crazyflie) -> Result<bool> {
    let memories: Vec<MemoryDevice> = cf.memory.get_memories(Some(MemoryType::OneWire))
      .into_iter()
      .cloned()
      .collect();

    for device in memories {
      // A memory that fails to parse (bad CRC etc) is skipped, as the nRF51 does
      let ow = match cf.memory.open_memory::<OwMemory>(device).await {
        Some(Ok(ow)) => ow,
        _ => continue,
      };

      let is_aideck = ow.vid() == AIDECK_VID
        && ow.pid() == AIDECK_PID
        && ow.elements().get("boardName").map(String::as_str) == Some(AIDECK_BOARD_NAME);
      cf.memory.close_memory(ow).await?;

      if is_aideck {
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

/// Where deck flashing reports to: the terminal with progress bars, as for
/// one Crazyflie, or one line per step with the Crazyflie's name in front and
/// no bars, for several Crazyflies flashed at the same time.
#[derive(Clone, Copy)]
pub enum DeckOutput<'a> {
    Terminal,
    Prefixed(&'a str),
}

impl DeckOutput<'_> {
    fn line(&self, message: &str) {
        match self {
            DeckOutput::Terminal => println!("{}", message),
            DeckOutput::Prefixed(name) => println!("{}: {}", name, message),
        }
    }

    fn progress_bar(&self, length: usize, label: &str) -> indicatif::ProgressBar {
        match self {
            DeckOutput::Terminal => get_progressbar(length, Some(label)),
            DeckOutput::Prefixed(_) => indicatif::ProgressBar::hidden(),
        }
    }

    fn finish(&self, bar: &indicatif::ProgressBar, message: String) {
        match self {
            DeckOutput::Terminal => finish_progress(bar, message),
            DeckOutput::Prefixed(_) => {
                bar.finish();
                self.line(&message);
            }
        }
    }
}

async fn flash_deck_ctrl(
    link_context: &LinkContext,
    uri: &str,
    toc_cache: ConfigTocCache,
    firmwares: &[Firmware],
    output: DeckOutput<'_>,
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
        let progress_bar = output.progress_bar(fw.data.len(), &label);
        let pb = progress_bar.clone();
        let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
        };
        raw.write_with_progress(address, &fw.data, progress_callback).await?;
        output.finish(&progress_bar, format!("DeckCtrl {} flashed successfully!", fw.file_type));
    }

    raw.write(DECK_CTRL_DFU_CMD_OFFSET, &[DECK_CTRL_DFU_CMD_ENTER_FIRMWARE]).await?;
    cf.disconnect().await;
    sleep(Duration::from_millis(DECK_CTRL_DFU_RESET_DELAY_MS)).await;

    Ok(())
}

pub async fn flash(link_context: &LinkContext, uri: &str, toc_cache: ConfigTocCache, firmware_upgrade: FirmwareUpgrade, cold: bool) -> Result<()> {

  let firmware_for_bootloader = firmware_upgrade.get_firmware_for_bootloader();
  let firmware_for_deckctrl = firmware_upgrade.get_firmware_for_deckctrl();
  let firmware_for_decks = firmware_upgrade.get_firmware_for_decks();

  // The nRF51 bootloader+softdevice is flashed through the same link, so the
  // bootloader has to be entered for it too even when it is all that was
  // selected.
  let has_nrf51_softdevice = firmware_upgrade.nrf51_softdevice().is_some();

  if !firmware_for_bootloader.is_empty() || has_nrf51_softdevice {
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

      upgrade_nrf51_softdevice(&mut cfloader, &firmware_upgrade, "nrf51-sd").await?;
      for firmware in firmware_for_bootloader {
        if firmware.target == "stm32" && firmware.file_type == "fw" {
          let start_address = start_address(&firmware, cfloader.stm32_info());
          let progress_bar = get_progressbar(firmware.data.len(), Some(firmware.target.as_str()));
          let pb = progress_bar.clone();
          let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
            pb.set_position(bytes_written as u64);
          };
          cfloader.flash_stm32_with_progress(start_address, &firmware.data, Some(progress_callback)).await?;
          finish_progress(&progress_bar, "STM32F405 flashed successfully!");
        }
        if firmware.target == "nrf51" && firmware.file_type == "fw" {
          let start_address = start_address(&firmware, cfloader.nrf51_info());
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

  flash_decks(link_context, uri, toc_cache, &firmware_upgrade, DeckOutput::Terminal).await
}

/// Flash the DeckCtrl and deck firmware of an upgrade, to a Crazyflie that
/// runs its firmware.
pub async fn flash_decks(
    link_context: &LinkContext,
    uri: &str,
    toc_cache: ConfigTocCache,
    firmware_upgrade: &FirmwareUpgrade,
    output: DeckOutput<'_>,
) -> Result<()> {
  let firmware_for_deckctrl = firmware_upgrade.get_firmware_for_deckctrl();
  let firmware_for_decks = firmware_upgrade.get_firmware_for_decks();

  if !firmware_for_deckctrl.is_empty() {
    flash_deck_ctrl(link_context, uri, toc_cache.clone(), &firmware_for_deckctrl, output).await?;
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
      DECK_REBOOT_DELAY_MS + AIDECK_BOOT_DELAY_MS
    } else {
      DECK_REBOOT_DELAY_MS
    };
    let mut flash_count_left = firmware_for_decks.len();
    cf.disconnect().await;

    for firmware in &firmware_for_decks {
      let cf = crazyflie_lib::Crazyflie::connect_from_uri(
          &link_context,
          uri,
          toc_cache.clone()
      ).await?;

      // Whether we wrote anything to the deck, and so whether the Crazyflie
      // has to be restarted for it to run the new firmware.
      let mut flashed = false;

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

          // Needed for the lighthouse deck, since we cannot reboot to bootloader
          // if the firmware matches the configuration.
          let up_to_date =
            !section.can_reset_to_bootloader() && !section.upgrade_required().await?;

          if up_to_date {
            output.line(&format!("Deck {} firmware up to date, skipping", section.name()));
          } else {
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

            let progress_bar = output.progress_bar(firmware.data.len(), section.name());
            let pb = progress_bar.clone();
            let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
              pb.set_position(bytes_written as u64);
            };
            section.flash_firmware_with_progress(&firmware.data, progress_callback).await?;
            output.finish(&progress_bar, format!("Deck {} firmware flashed successfully!", section.name()));
            flashed = true;
          }
        }

        cf.disconnect().await;

      }

      flash_count_left = flash_count_left - 1;

      if flashed {
        reboot(&link_context, uri).await?;

        if flash_count_left > 0 {
            output.line("Restarting Crazyflie...");
            sleep(Duration::from_millis(delay)).await;
        }
      }
    }
  }

  Ok(())
}

// ---------------------------------------------------------------------------
// nRF51 softdevice upgrade
//
// Release zips carry a combined nRF51 bootloader+softdevice binary, and the
// nRF51 firmware declares which softdevice it needs (`requires: [sd-s130]`).
// The bundle declares what it brings (`provides: [sd-s130]`). The device
// reports which softdevice it is running through its bootloader's start page.
//
// Unlike cfloader we only ever move forwards: if the archive would take the
// device back to an older softdevice or an older bootloader we refuse instead
// of flashing, so we never need to carry an old softdevice of our own.
// ---------------------------------------------------------------------------

/// How long to wait for the nRF51 to come back after its bootloader has been
/// replaced and it has been restarted.
const NRF51_RESTART_TIMEOUT: Duration = Duration::from_secs(10);

/// nRF51 bootloader start page when running the S110 softdevice.
const NRF51_START_PAGE_S110: u16 = 88;
/// nRF51 bootloader start page when running the S130 softdevice.
const NRF51_START_PAGE_S130: u16 = 108;

const NRF51_SOFTDEVICE_S110: &str = "sd-s110";
const NRF51_SOFTDEVICE_S130: &str = "sd-s130";

/// Which softdevice the nRF51 is running, worked out from where its
/// bootloader says the firmware area starts.
fn nrf51_softdevice_from_start_page(start_page: u16) -> Option<&'static str> {
    match start_page {
        NRF51_START_PAGE_S110 => Some(NRF51_SOFTDEVICE_S110),
        NRF51_START_PAGE_S130 => Some(NRF51_SOFTDEVICE_S130),
        _ => None,
    }
}

/// A dotted numeric version, as both the bootloader and the manifest use.
///
/// The bootloader derives its numbers from the git tag of its repository
/// (`2024.10` becomes major 2024, minor 10), and the manifest names the same
/// release, so the two are directly comparable. Missing components count as
/// zero, so `2024.10` and `2024.10.0` are equal.
fn parse_version(version: &str) -> Option<Vec<u32>> {
    // A bootloader built from a dirty tree reports a `+dev` suffix.
    let version = version.split('+').next().unwrap_or(version).trim();
    if version.is_empty() {
        return None;
    }

    version
        .split('.')
        .map(|part| part.parse::<u32>().ok())
        .collect()
}

/// Compare two dotted versions, `None` if either can't be read as one.
fn compare_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let (left, right) = (parse_version(left)?, parse_version(right)?);
    let width = left.len().max(right.len());

    for i in 0..width {
        let l = left.get(i).copied().unwrap_or(0);
        let r = right.get(i).copied().unwrap_or(0);
        if l != r {
            return Some(l.cmp(&r));
        }
    }

    Some(std::cmp::Ordering::Equal)
}

/// What to do about the nRF51 bootloader+softdevice bundle before flashing
/// the nRF51 firmware.
#[derive(Debug, PartialEq, Eq)]
enum SoftdeviceAction {
    /// Nothing to do.
    Skip,
    /// Flash the bundle first.
    Flash,
}

/// Decide whether the bundled bootloader+softdevice has to be flashed.
///
/// `current_sd` is what the device runs, `required_sd` what the archive's
/// nRF51 firmware needs, `provided_sd` what the bundle brings. `current_bl`
/// and `provided_bl` are the bootloader versions on the device and in the
/// archive.
///
/// Returns an error rather than flashing whenever that would move the device
/// backwards, or when the archive can't satisfy the firmware it ships.
fn decide_softdevice_action(
    has_bundle: bool,
    current_sd: Option<&str>,
    required_sd: Option<&str>,
    provided_sd: Option<&str>,
    current_bl: Option<&str>,
    provided_bl: Option<&str>,
) -> Result<SoftdeviceAction> {
    if !has_bundle {
        // Nothing to flash. If the firmware needs a softdevice the device
        // isn't running, we can't help it.
        if let (Some(required), Some(current)) = (required_sd, current_sd) {
            if required != current {
                bail!(CliError::InvalidValue(format!(
                    "the nRF51 firmware needs {} but the device runs {}, and the archive has no \
                     {} binary to upgrade it with",
                    required, current, crate::utils::firmware::NRF51_SOFTDEVICE_TYPE
                )));
            }
        }
        return Ok(SoftdeviceAction::Skip);
    }

    // The firmware needs a softdevice that neither the device nor the bundle
    // can give it.
    if let Some(required) = required_sd {
        let satisfied = current_sd == Some(required) || provided_sd == Some(required);
        if !satisfied {
            bail!(CliError::InvalidValue(format!(
                "cannot flash nRF51: firmware needs {}, device runs {} and the archive provides {}",
                required,
                current_sd.unwrap_or("an unknown softdevice"),
                provided_sd.unwrap_or("none"),
            )));
        }
    }

    // A softdevice change. We only go forwards, so S110 -> S130 is an
    // upgrade and the other direction is refused.
    if let (Some(current), Some(provided)) = (current_sd, provided_sd) {
        if current != provided {
            if current == NRF51_SOFTDEVICE_S130 && provided == NRF51_SOFTDEVICE_S110 {
                bail!(CliError::InvalidValue(format!(
                    "refusing to downgrade the nRF51 softdevice from {} to {}",
                    current, provided
                )));
            }
            return Ok(SoftdeviceAction::Flash);
        }
    }

    // Same softdevice, so this comes down to the bootloader version.
    match (current_bl, provided_bl) {
        // A bundle given with --bin has no release version, only "custom".
        // It was asked for explicitly, so it is flashed whatever the device
        // runs; this is how a development bootloader gets onto a Crazyflie.
        (_, Some(provided)) if parse_version(provided).is_none() => Ok(SoftdeviceAction::Flash),
        (Some(current), Some(provided)) => match compare_versions(provided, current) {
            Some(std::cmp::Ordering::Greater) => Ok(SoftdeviceAction::Flash),
            // A device that is already current, or newer than the archive,
            // is left alone; so is one whose version we cannot read, rather
            // than guessing at it.
            Some(_) | None => Ok(SoftdeviceAction::Skip),
        },
        // The original bootloader doesn't report a version. Anything the
        // archive offers is newer than that.
        (None, Some(_)) => Ok(SoftdeviceAction::Flash),
        _ => Ok(SoftdeviceAction::Skip),
    }
}

/// Flash the nRF51 bootloader+softdevice bundle if the archive has one and
/// the device needs it.
///
/// Returns a loader connected to whichever bootloader is running afterwards:
/// the same one if nothing was flashed, or the newly flashed one, which has
/// to be reconnected to.
///
/// `label` goes in front of the progress bar.
pub(crate) async fn upgrade_nrf51_softdevice(
    cfloader: &mut cfloader::CFLoader,
    firmware_upgrade: &FirmwareUpgrade,
    label: &str,
) -> Result<()> {
    let bundle = firmware_upgrade.nrf51_softdevice();
    let provided_sd = bundle.as_ref().and_then(|fw| fw.provides.first().cloned());
    let provided_bl = bundle.as_ref().map(|fw| fw.version.clone());

    let nrf51_info = cfloader.nrf51_info();
    let current_sd = nrf51_softdevice_from_start_page(nrf51_info.flash_start());
    let current_bl = nrf51_info.firmware_version().map(|v| v.to_string());

    // Deciding needs the device to be in its bootloader, so by the time we
    // can refuse an upgrade the Crazyflie has already been restarted into it.
    // Nothing has been written at this point, so put it back into its
    // firmware rather than leaving it stranded in the bootloader.
    let decision = firmware_upgrade
        .required_nrf51_softdevice()
        .and_then(|required_sd| {
            decide_softdevice_action(
                bundle.is_some(),
                current_sd,
                required_sd.as_deref(),
                provided_sd.as_deref(),
                current_bl.as_deref(),
                provided_bl.as_deref(),
            )
        });

    let action = match decision {
        Ok(action) => action,
        Err(e) => {
            if let Err(reset_error) = cfloader.reset_to_firmware().await {
                eprintln!(
                    "Warning: could not restart the Crazyflie into its firmware: {}",
                    reset_error
                );
            }
            return Err(e);
        }
    };

    if action == SoftdeviceAction::Skip {
        return Ok(());
    }

    let bundle = bundle.expect("a bundle is present whenever we decide to flash one");

    let page_size = nrf51_info.page_size() as usize;
    let flash_pages = nrf51_info.n_flash_page() as usize;
    let firmware_start = nrf51_info.flash_start() as usize;

    if page_size == 0 {
        bail!("nRF51 bootloader reported a page size of 0");
    }
    let bundle_pages = bundle.data.len().div_ceil(page_size);
    if bundle_pages > flash_pages {
        bail!(
            "{} is {} bytes, which does not fit in the nRF51's {} pages of {} bytes",
            bundle.file_name,
            bundle.data.len(),
            flash_pages,
            page_size
        );
    }

    // Replacing the bootloader erases part of the firmware. Erase the
    // firmware's first page up front so that a bootloader interrupted
    // half-way can't jump into what is left of it, and stays in bootloader
    // mode where we can reach it again.
    cfloader
        .flash_nrf51((firmware_start * page_size) as u32, &vec![0xFF; page_size])
        .await?;

    // The bundle lives at the very end of flash.
    let start_page = flash_pages - bundle_pages;
    let progress_bar = get_progressbar(bundle.data.len(), Some(label));
    let pb = progress_bar.clone();
    let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
        pb.set_position(bytes_written as u64);
    };
    cfloader
        .flash_nrf51_with_progress(
            (start_page * page_size) as u32,
            &bundle.data,
            Some(progress_callback),
        )
        .await?;
    finish_progress(&progress_bar, "nRF51 bootloader+softdevice flashed successfully!");

    // Restart so the bootloader we just wrote takes over, staying in the
    // bootloader rather than starting the firmware we erased a page of. The
    // link is connectionless, so it survives the restart; the cached info
    // does not, since the new bootloader may report a different flash layout.
    cfloader.reset_to_bootloader().await?;
    cfloader.refresh_info(NRF51_RESTART_TIMEOUT).await?;

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    const S130: Option<&str> = Some(NRF51_SOFTDEVICE_S130);

    #[test]
    fn raw_bundle_is_flashed_over_a_versioned_bootloader() {
        let action = decide_softdevice_action(true, S130, None, None, Some("2024.10.0"), Some("custom")).unwrap();
        assert_eq!(action, SoftdeviceAction::Flash);
    }

    #[test]
    fn release_bundle_is_skipped_when_bootloader_is_current() {
        let action = decide_softdevice_action(true, S130, Some(NRF51_SOFTDEVICE_S130), S130, Some("2024.10.0"), Some("2024.10")).unwrap();
        assert_eq!(action, SoftdeviceAction::Skip);
    }

    #[test]
    fn release_bundle_is_flashed_when_newer() {
        let action = decide_softdevice_action(true, S130, Some(NRF51_SOFTDEVICE_S130), S130, Some("2024.10.0"), Some("2026.08")).unwrap();
        assert_eq!(action, SoftdeviceAction::Flash);
    }
}
