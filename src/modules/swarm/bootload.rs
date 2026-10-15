//! `cfcli swarm bootload`: the bootloaders of a swarm, and flashing it.
//!
//! `flash` flashes the Crazyflies one after another over unicast, the same
//! way `cfcli bootload flash` flashes one. A unicast flash keeps the radio
//! busy, so flashing several at once wouldn't be faster. `info` shows which
//! Crazyflies have bootloaders that could also take broadcast flashing,
//! where every Crazyflie receives each image at the same time.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use tabled::Tabled;

use super::runner::{csv_row_for, split, Runner, SwarmRow};
use crate::error::CliError;
use crate::modules::bootloader;
use crate::utils::display::{csv_row, print_table, table};
use crate::utils::firmware::FirmwareUpgrade;
use crate::utils::flash_source;
use crate::{ConfigTocCache, SwarmBootloadCommands, SwarmFlashParameters};

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
        SwarmBootloadCommands::Flash(params) => flash(runner, params, toc_cache, non_interactive).await,
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
    params: &SwarmFlashParameters,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
) -> Result<()> {
    let source = &params.source;
    // Everything that comes from the command line first, before any
    // Crazyflie is touched.
    let only = params.platform.as_deref().map(flash_source::platform_name).transpose()?;
    let release = flash_source::release(&source.release, non_interactive).await?;
    let bins = flash_source::bins(&source.bin, &source.targets, non_interactive)?;

    // The platform decides which files of a release a Crazyflie gets.
    let platforms = runner
        .connected("Reading", async |cf, _| Ok(cf.platform.device_type_name().await?))
        .await;
    let to_flash: Vec<(usize, &str)> = platforms
        .iter()
        .enumerate()
        .filter_map(|(i, platform)| platform.as_ref().ok().map(|p| (i, p.as_str())))
        .filter(|(_, platform)| only.is_none_or(|only| *platform == only))
        .collect();
    if let Some(only) = only {
        if to_flash.is_empty() && platforms.iter().any(Result::is_ok) {
            bail!(CliError::NotFound(format!("Crazyflies of platform {} in the swarm", only)));
        }
    }

    // STM32 and nRF51 images are built for one platform.
    let named: Vec<(&str, &str)> = to_flash.iter().map(|(i, p)| (runner.targets[*i].name.as_str(), *p)).collect();
    check_one_platform(&flash_source::platform_bound(&bins), &named)?;

    // The files for every platform, all ready before anything is flashed.
    let mut distinct: Vec<&str> = to_flash.iter().map(|(_, platform)| *platform).collect();
    distinct.sort_unstable();
    distinct.dedup();
    let mut upgrades: Vec<(String, FirmwareUpgrade)> = Vec::new();
    let mut unprepared = 0;
    for platform in &distinct {
        let platform = platform.to_string();
        match FirmwareUpgrade::new(&platform, &release, &source.zip, &bins).await {
            Ok(upgrade) => upgrades.push((platform, upgrade)),
            Err(e) => {
                eprintln!("{}: {:#}", platform, e);
                unprepared += 1;
            }
        }
    }
    if unprepared > 0 {
        let hint = if distinct.len() > 1 { ". Flash one platform at a time with --platform" } else { "" };
        bail!(CliError::InvalidValue(format!(
            "the firmware can't be prepared for {} of the platforms to flash, so nothing was flashed{}",
            unprepared, hint
        )));
    }

    // The targets are picked once, from those of the first platform.
    if let Some((_, first)) = upgrades.first() {
        let selected = flash_source::targets(first, &source.targets, non_interactive)?;
        for (_, upgrade) in upgrades.iter_mut() {
            upgrade.filter_targets(&selected);
        }
    }

    // An nRF51 bootloader from a file is asked about once, for the whole
    // swarm. Being an nRF51 --bin it limits the swarm to one platform, so
    // the first upgrade is the only one.
    if let Some((_, first)) = upgrades.first() {
        if !flash_source::confirm_nrf51_bootloader(first, to_flash.len(), source.accept_bootloader_risk, non_interactive)? {
            println!("Nothing flashed");
            return Ok(());
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
        let Some((_, upgrade)) = upgrades.iter().find(|(p, _)| *p == platform) else {
            // Left out by --platform.
            results.push(Ok("skipped".to_string()));
            continue;
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

/// Refuse STM32 and nRF51 images (`bound`, the `--bin` keys) for Crazyflies
/// of more than one platform (`to_flash`: name and platform each).
fn check_one_platform(bound: &[String], to_flash: &[(&str, &str)]) -> Result<()> {
    if bound.is_empty() {
        return Ok(());
    }
    let mut by_platform: Vec<(&str, Vec<&str>)> = Vec::new();
    for (name, platform) in to_flash {
        match by_platform.iter_mut().find(|(p, _)| p == platform) {
            Some((_, names)) => names.push(name),
            None => by_platform.push((platform, vec![name])),
        }
    }
    if by_platform.len() <= 1 {
        return Ok(());
    }
    let groups: Vec<String> =
        by_platform.iter().map(|(platform, names)| format!("{} ({})", platform, names.join(", "))).collect();
    bail!(CliError::InvalidValue(format!(
        "--bin {} {} built for one platform, but the Crazyflies to flash are {}. Flash one platform at a \
         time with --platform, or use --release, which has the files for every platform",
        bound.join(", "),
        if bound.len() == 1 { "is" } else { "are" },
        groups.join(" and ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_bound_images_need_one_platform() {
        let bound = vec!["stm32-fw".to_string()];
        let mixed = [("CF-01", "Crazyflie 2.1"), ("CF-02", "Crazyflie 2.1 Brushless"), ("CF-03", "Crazyflie 2.1")];
        let err = check_one_platform(&bound, &mixed).unwrap_err();
        let message = format!("{:#}", err);
        assert!(
            message.contains("Crazyflie 2.1 (CF-01, CF-03) and Crazyflie 2.1 Brushless (CF-02)"),
            "{}",
            message
        );

        // One platform, or no STM32/nRF51 image, is fine.
        assert!(check_one_platform(&bound, &mixed[..1]).is_ok());
        assert!(check_one_platform(&[], &mixed).is_ok());
    }
}
