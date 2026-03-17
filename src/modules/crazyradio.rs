use anyhow::{bail, Result};
use crazyradio::{Channel, Crazyradio, Datarate};

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
