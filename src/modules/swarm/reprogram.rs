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
use crazyflie_lib::crazyflie_link::LinkContext;
use crazyflie_lib::Crazyflie;

use super::runner::Runner;
use super::store::{Swarm, Unit};
use crate::error::CliError;
use crate::modules::{bootloader, config};
use crate::utils::radio::RadioUri;
use crate::{Config, ConfigTocCache};

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
    link_context: &LinkContext,
    config: &Config,
    toc_cache: ConfigTocCache,
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
    let runner = Runner::new(link_context, config, toc_cache.clone(), &links, &[0, 1]).await?;
    let (old, new) = (&runner.targets[0], &runner.targets[1]);

    let cf = match usb {
        Some(usb) => Crazyflie::connect_from_uri(link_context, usb, toc_cache)
            .await
            .context(CliError::Connection(format!("connecting to {}", usb)))?,
        None => runner.connect(old).await?,
    };
    let channel = (to.channel != from.channel).then_some(to.channel);
    let address = if to.address != from.address { Some(crate::decode_address(&to.address)?) } else { None };
    let written = config::write_radio(&cf, channel, address).await;
    cf.disconnect().await;
    written.context("not reprogrammed")?;
    println!("{}: reprogrammed {} -> {}", planned.name, planned.from, planned.uri);

    bootloader::reboot(link_context, &old.link_uri)
        .await
        .context("reprogrammed, but not rebooted (it takes the new address when restarted)")?;
    if new.wait_for(link_context).await {
        println!("{}: rebooted, answers on {}", planned.name, planned.uri);
        return Ok(());
    }
    if old.answers(link_context).await {
        bail!("still answers on {} after the reboot, the new address didn't take", planned.from);
    }
    bail!(CliError::Connection(format!(
        "reprogrammed, but doesn't answer on {} (or {}) after the reboot",
        planned.uri, planned.from
    )))
}
