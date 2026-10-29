//! Reprogramming the Crazyflies `swarm config add` adds with
//! `--base-address`, `--set-address` or `--set-channel`.
//!
//! The firmware only reads the radio channel and address from its EEPROM
//! config at boot. So the new ones are written (over USB, or over the radio
//! on the old ones) and read back, the Crazyflie is rebooted, and then it is
//! looked for on the new ones; it is only added once it answers there. The
//! reboot always goes over the radio, on the old channel and address: the
//! nRF51 handles it, and it only gets that command from the radio, not from
//! USB. That is why reprogramming needs a Crazyradio even over USB.

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_connection_server::Client;
use crazyflie_lib::crazyflie_link::LinkContext;

use super::runner::Runner;
use super::store::{Swarm, Unit};
use crate::error::CliError;
use crate::modules::config;
use crate::utils::radio::RadioUri;
use crate::Config;

/// A Crazyflie about to be added.
pub struct Planned {
    /// Where it was found.
    pub from: String,
    /// The URI it is added with: the same, or where it is reprogrammed to.
    pub uri: String,
    pub name: String,
}

/// Reprogram a Crazyflie from `planned.from` to `planned.uri` and check that
/// it answers there. `usb` is the USB URI it was found on, if any.
pub async fn reprogram(
    client: &Client,
    link_context: &LinkContext,
    config: &Config,
    usb: Option<&str>,
    planned: &Planned,
) -> Result<()> {
    let from = RadioUri::parse(&planned.from)?.ok_or_else(|| anyhow!("{} isn't a radio URI", planned.from))?;
    let to = RadioUri::parse(&planned.uri)?.ok_or_else(|| anyhow!("{} isn't a radio URI", planned.uri))?;
    if from.channel == to.channel && from.address == to.address {
        println!("{}: already on {}, nothing to reprogram", planned.name, planned.uri);
        return Ok(());
    }

    // The Crazyflie on its old and its new link, each given a Crazyradio.
    let mut links = Swarm::new(String::new(), None);
    for uri in [&planned.from, &planned.uri] {
        links.units.push(Unit { uri: uri.clone(), name: planned.name.clone(), description: None, extra: Default::default() });
    }
    let runner = Runner::new(client, link_context, config, &links, &[0, 1]).await?;
    let (old, new) = (&runner.targets[0], &runner.targets[1]);

    let channel = (to.channel != from.channel).then_some(to.channel);
    let address = if to.address != from.address { Some(crate::decode_address(&to.address)?) } else { None };
    // Over USB, or over the radio on the old link
    let cf = match usb {
        Some(usb) => client.attach(usb).await.context(CliError::Connection(format!("connecting to {}", usb)))?,
        None => runner.attach(old).await?.cf,
    };
    let written = config::write_radio(&cf, channel, address).await;
    // Gone before the reboot, which opens a link of its own
    let lib = cf.lib();
    cf.detach();
    if let Some(lib) = lib {
        lib.disconnect().await;
    }
    written.context("not reprogrammed")?;
    println!("{}: reprogrammed {} -> {}", planned.name, planned.from, planned.uri);

    // The reboot returns once it answers on the new link
    let rebooted = runner.client().reboot_to(&old.link_uri, &new.link_uri).await;
    if rebooted.is_ok() || runner.answers(new).await {
        println!("{}: rebooted, answers on {}", planned.name, planned.uri);
        return Ok(());
    }
    if runner.answers(old).await {
        bail!("still answers on {} after the reboot, the new address didn't take", planned.from);
    }
    bail!(CliError::Connection(format!(
        "reprogrammed, but doesn't answer on {} (or {}) after the reboot",
        planned.uri, planned.from
    )))
}
