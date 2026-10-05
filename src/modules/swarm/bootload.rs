//! `cfcli swarm bootload`: the bootloaders of a swarm, and flashing it.
//!
//! `flash` flashes the Crazyflies one after another over unicast, the same
//! way `cfcli bootload flash` flashes one. A unicast flash keeps the radio
//! busy, so flashing several at once wouldn't be faster. `info` shows which
//! Crazyflies have bootloaders that could also take broadcast flashing,
//! where every Crazyflie receives each image at the same time.

use std::time::Duration;

use anyhow::{anyhow, Result};
use tabled::Tabled;

use super::runner::{csv_row_for, split, Runner, SwarmRow};
use crate::error::CliError;
use crate::modules::bootloader;
use crate::utils::display::{csv_row, print_table, table};
use crate::utils::firmware::FirmwareUpgrade;
use crate::utils::flash_source;
use crate::{ConfigTocCache, FirmwareSourceArgs, SwarmBootloadCommands};

/// How long reading the bootloader versions of one Crazyflie may take.
const INFO_TIMEOUT: Duration = Duration::from_secs(15);
/// The first bootloader protocol version with broadcast flashing, on both
/// the nRF51 and the STM32.
const BROADCAST_PROTOCOL: u8 = 0x11;

pub async fn bootload(
    runner: &Runner<'_>,
    command: &SwarmBootloadCommands,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
    csv: bool,
) -> Result<()> {
    match command {
        SwarmBootloadCommands::Info => info(runner, csv).await,
        SwarmBootloadCommands::Flash(params) => flash(runner, &params.source, toc_cache, non_interactive).await,
    }
}

/// One row of `swarm bootload info`.
#[derive(Tabled)]
struct InfoRow {
    #[tabled(rename = "nRF51 bootloader")]
    nrf51: String,
    #[tabled(rename = "STM32 bootloader")]
    stm32: String,
    #[tabled(rename = "Broadcast")]
    broadcast: String,
}

async fn info(runner: &Runner<'_>, csv: bool) -> Result<()> {
    let link_context = runner.link_context();
    let results = runner
        .each("Reading", async |t| {
            // Restarting a Crazyflie that isn't there fails with an unhelpful
            // link error.
            t.check_answers(link_context).await?;
            tokio::time::timeout(INFO_TIMEOUT, bootloader::bootloader_versions(link_context, &t.link_uri))
                .await
                .map_err(|_| {
                    anyhow!(CliError::Timeout(format!(
                        "{} didn't finish within {} s",
                        t.link_uri,
                        INFO_TIMEOUT.as_secs()
                    )))
                })?
        })
        .await;
    let (done, outcome) = split(results);
    let broadcast = |stm32: u8, nrf51: u8| if stm32 >= BROADCAST_PROTOCOL && nrf51 >= BROADCAST_PROTOCOL { "yes" } else { "no" };
    if csv {
        csv_row(&["cf", "uri", "nrf51_protocol", "stm32_protocol", "broadcast"]);
        for (i, (stm32, nrf51)) in &done {
            let (nrf51_s, stm32_s) = (format!("0x{:02X}", nrf51), format!("0x{:02X}", stm32));
            csv_row_for(&runner.targets[*i], &[&nrf51_s, &stm32_s, broadcast(*stm32, *nrf51)]);
        }
    } else if !done.is_empty() {
        let rows: Vec<_> = done
            .into_iter()
            .map(|(i, (stm32, nrf51))| SwarmRow {
                cf: runner.targets[i].name.clone(),
                row: InfoRow {
                    nrf51: format!("0x{:02X}", nrf51),
                    stm32: format!("0x{:02X}", stm32),
                    broadcast: broadcast(stm32, nrf51).to_string(),
                },
            })
            .collect();
        print_table(&table(&rows));
    }
    outcome.print_failures(&runner.targets);
    outcome.finish()
}

/// One row of the `swarm bootload flash` summary.
#[derive(Tabled)]
struct FlashRow {
    #[tabled(rename = "CF")]
    name: String,
    #[tabled(rename = "Platform")]
    platform: String,
    #[tabled(rename = "Result")]
    result: String,
}

async fn flash(
    runner: &Runner<'_>,
    source: &FirmwareSourceArgs,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
) -> Result<()> {
    // Everything that comes from the command line first, before any
    // Crazyflie is touched.
    let release = flash_source::release(&source.release, non_interactive).await?;
    let bins = flash_source::bins(&source.bin, &source.targets, non_interactive)?;

    // The platform decides which files of a release a Crazyflie gets.
    let platforms = runner
        .connected("Reading", async |cf, _| Ok(cf.platform.device_type_name().await?))
        .await;
    let mut upgrades: Vec<(String, Result<FirmwareUpgrade>)> = Vec::new();
    for platform in platforms.iter().flatten() {
        if !upgrades.iter().any(|(p, _)| p == platform) {
            let upgrade = FirmwareUpgrade::new(platform, &release, &source.zip, &bins).await;
            upgrades.push((platform.clone(), upgrade));
        }
    }

    // The targets are picked once, from those of the first platform.
    if let Some(first) = upgrades.iter().find_map(|(_, upgrade)| upgrade.as_ref().ok()) {
        let selected = flash_source::targets(first, &source.targets, non_interactive)?;
        for (_, upgrade) in upgrades.iter_mut() {
            if let Ok(upgrade) = upgrade {
                upgrade.filter_targets(&selected);
            }
        }
    }

    // One Crazyflie after another.
    let link_context = runner.link_context();
    let total = runner.targets.len();
    let mut results: Vec<Result<String>> = Vec::with_capacity(total);
    let mut shown_platforms = Vec::with_capacity(total);
    for (i, (target, platform)) in runner.targets.iter().zip(platforms).enumerate() {
        let platform = match platform {
            Ok(platform) => platform,
            Err(e) => {
                shown_platforms.push(String::new());
                results.push(Err(e));
                continue;
            }
        };
        shown_platforms.push(platform.clone());
        let upgrade = match upgrades.iter().find(|(p, _)| *p == platform).map(|(_, u)| u) {
            Some(Ok(upgrade)) => upgrade,
            Some(Err(e)) => {
                results.push(Err(anyhow!("{:#}", e)));
                continue;
            }
            None => unreachable!("every platform found has an upgrade"),
        };
        if upgrade.get_target_and_types().is_empty() {
            results.push(Ok(format!("nothing to flash for {}", platform)));
            continue;
        }
        println!();
        println!("{} ({} of {}, {}): {}", target.name, i + 1, total, platform, target.uri);
        let flashed = bootloader::flash(link_context, &target.link_uri, toc_cache.clone(), upgrade.clone(), false).await;
        results.push(flashed.map(|()| "flashed".to_string()));
    }

    let rows: Vec<FlashRow> = runner
        .targets
        .iter()
        .zip(&shown_platforms)
        .zip(&results)
        .map(|((target, platform), result)| FlashRow {
            name: target.name.clone(),
            platform: platform.clone(),
            result: match result {
                Ok(done) => done.clone(),
                Err(e) => format!("{:#}", e),
            },
        })
        .collect();
    println!();
    print_table(&table(&rows));
    let (_, outcome) = split(results);
    outcome.finish()
}
