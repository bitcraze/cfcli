use anyhow::{bail, Result};
use crazyradio::{Channel, Crazyradio, Datarate};
use std::time::Duration;

const CRAZYRADIO_VID: u16 = 0x1915;
const CRAZYRADIO_PID: u16 = 0x7777;

pub fn list() -> Result<()> {
    let devices = rusb::devices()?;

    let mut found = 0u32;
    for device in devices.iter() {
        let desc = device.device_descriptor()?;
        if desc.vendor_id() != CRAZYRADIO_VID || desc.product_id() != CRAZYRADIO_PID {
            continue;
        }

        let handle = match device.open() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("Crazyradio #{found}: failed to open ({e})");
                found += 1;
                continue;
            }
        };

        let languages = handle.read_languages(Duration::from_secs(1)).unwrap_or_default();
        let lang = languages.first().copied();

        let serial = lang
            .and_then(|l| handle.read_serial_number_string(l, &desc, Duration::from_secs(1)).ok())
            .unwrap_or_else(|| "N/A".to_string());

        let product = lang
            .and_then(|l| handle.read_product_string(l, &desc, Duration::from_secs(1)).ok())
            .unwrap_or_else(|| "Unknown".to_string());

        let fw_version = desc.device_version();

        let bus = device.bus_number();
        let address = device.address();
        let port_numbers = device.port_numbers().unwrap_or_default();
        let port_path: String = port_numbers
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(".");

        println!("Crazyradio #{found}");
        println!("  Serial:    {serial}");
        println!("  Product:   {product}");
        println!("  Firmware:  {}.{}", fw_version.major(), fw_version.minor() * 10 + fw_version.sub_minor());
        println!("  USB bus:   {bus}, address: {address}, port path: {port_path}");
        println!();

        found += 1;
    }

    if found == 0 {
        println!("No Crazyradios found.");
    } else {
        println!("{found} Crazyradio(s) found.");
    }

    Ok(())
}

pub async fn sniff(radio: usize, channel: u8, datarate: u8, address: &[u8; 5]) -> Result<()> {
    let channel = Channel::from_number(channel)
        .map_err(|e| anyhow::anyhow!("Invalid channel: {}", e))?;

    let datarate = match datarate {
        0 => Datarate::Dr250K,
        1 => Datarate::Dr1M,
        2 => Datarate::Dr2M,
        _ => bail!("Invalid datarate: {}. Use 0=250K, 1=1M, 2=2M", datarate),
    };

    let mut cr = Crazyradio::open_nth_async(radio).await
        .map_err(|e| anyhow::anyhow!("Failed to open Crazyradio {}: {}", radio, e))?;

    cr.set_channel(channel)
        .map_err(|e| anyhow::anyhow!("Failed to set channel: {}", e))?;
    cr.set_datarate(datarate)
        .map_err(|e| anyhow::anyhow!("Failed to set datarate: {}", e))?;
    cr.set_address(address)
        .map_err(|e| anyhow::anyhow!("Failed to set address: {}", e))?;
    cr.set_sniffer_address(0, address)
        .map_err(|e| anyhow::anyhow!("Failed to set sniffer address: {}", e))?;

    let channel_num: u8 = channel.into();
    let datarate_str = match datarate {
        Datarate::Dr250K => "250K",
        Datarate::Dr1M => "1M",
        Datarate::Dr2M => "2M",
    };
    println!(
        "Entering sniffer mode on channel {}, {}, address {:02X?}",
        channel_num,
        datarate_str,
        address,
    );

    let (receiver, _sender) = cr.enter_sniffer_mode_async().await
        .map_err(|e| anyhow::anyhow!("Failed to enter sniffer mode: {}", e))?;

    loop {
        match receiver.recv().await {
            Some(Ok(pkt)) => {
                println!(
                    "pipe:{} rssi:{}dBm ts:{}us len:{} data:{:02x?}",
                    pkt.pipe,
                    pkt.rssi_dbm,
                    pkt.timestamp_us,
                    pkt.payload.len(),
                    &pkt.payload,
                );
            }
            Some(Err(e)) => {
                eprintln!("Sniffer error: {:?}", e);
                break;
            }
            None => {
                println!("Sniffer session ended");
                break;
            }
        }
    }

    Ok(())
}
