use tokio::time::{sleep, Duration};
use crazyflie_link::{Connection, Packet};

const TARGET_NRF51: u8 = 0xFE;

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum BootloaderCommand {
  AllOff = 0x01,
  SysOff = 0x02,
  SysOn = 0x03,
  ResetInit = 0xFF,
  Reset = 0xF0,
}

async fn send_command(link: &Connection, cmd: BootloaderCommand, data: Option<&[u8]>) -> Result<(), Box<dyn std::error::Error>> {

    let mut command = vec![0xFF, TARGET_NRF51, cmd as u8];
    if let Some(d) = data {
        command.extend_from_slice(d);
    }

    let packet: Packet = command.into();
    link.send_packet(packet).await?;

    Ok(())
}

pub async fn reboot(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<(), Box<dyn std::error::Error>> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::ResetInit, None).await?;
  send_command(&link, BootloaderCommand::Reset, Some(&[0x01])).await?; // Reset to firmware
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

pub async fn power_off(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<(), Box<dyn std::error::Error>> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::AllOff, None).await?;
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

pub async fn sysoff(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<(), Box<dyn std::error::Error>> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::SysOff, None).await?;
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}

pub async fn syson(link_context: &crazyflie_link::LinkContext, uri: &str,) -> Result<(), Box<dyn std::error::Error>> {

  let link = link_context.open_link(uri).await?;
  send_command(&link, BootloaderCommand::SysOn, None).await?;
  sleep(Duration::from_millis(500)).await;
  
  Ok(())
}