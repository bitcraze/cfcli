//! `cfcli swarm bootload`: the bootloaders of a swarm, and flashing it.
//!
//! `flash` sends the STM32 and nRF51 firmware by broadcast to the Crazyflies
//! whose bootloaders take it, so that they all receive each image at the
//! same time, and flashes the others one after another (see [`broadcast`]).
//! With `--unicast` it flashes every Crazyflie one after another, the same
//! way `cfcli bootload flash` flashes one. A unicast flash keeps the radio
//! busy, so flashing several at once wouldn't be faster. `info` shows which
//! Crazyflies have bootloaders that take broadcast.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_connection_server::FlashProgress;
use tabled::Tabled;

use super::broadcast::{self, Job};
use super::runner::{csv_row_for, split, Runner, SwarmRow};
use crate::error::CliError;
use crate::modules::bootloader;
use crate::utils::display::{csv_row, finish_progress, get_progressbar, print_table, table};
use crate::utils::firmware::FirmwareUpgrade;
use crate::utils::flash_source;
use crate::{ConfigTocCache, SwarmBootloadCommands, SwarmFlashParameters};

/// Crazyflies in their bootloaders at the same time on each Crazyradio. A
/// link to a bootloader polls without letting up, and a Crazyflie that is
/// restarting leaves the Crazyradio retrying, so more of them at once slow
/// each other down until reading times out.
pub(super) const BOOTLOADER_PER_RADIO: usize = 2;
/// The first bootloader protocol version with broadcast flashing, on both
/// the nRF51 and the STM32.
pub(super) const BROADCAST_PROTOCOL: u8 = 0x11;

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
    let link_context = runner.direct("swarm bootload info")?;
    // Not cut short by a timeout: that could leave a Crazyflie in its
    // bootloader, and every step of reading has its own.
    let results = runner
        .each_at_most("Reading", BOOTLOADER_PER_RADIO, async |t| {
            // Restarting a Crazyflie that isn't there fails with an unhelpful
            // link error.
            runner.check_answers(t).await?;
            bootloader::bootloader_versions(link_context, &t.link_uri).await
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
    if params.unicast && !runner.is_direct() {
        bail!(CliError::Connection(
            "--unicast does not go through a connection server: the server flashes the Crazyflies whose \
             bootloaders don't take broadcast one after another. Use --connection direct"
                .to_string()
        ));
    }
    // Everything that comes from the command line first, before any
    // Crazyflie is touched.
    let only = params.platform.as_deref().map(flash_source::platform_name).transpose()?;
    let release = flash_source::release(&source.release, non_interactive).await?;
    let bins = flash_source::bins(&source.bin, &source.targets, non_interactive)?;

    // The platform decides which files of a release a Crazyflie gets. One
    // that is asleep is woken up to read it, since flashing restarts it anyway.
    let platforms = runner
        .each("Reading", async |target| {
            let attached = match runner.attach(target).await {
                Err(e) if format!("{:#}", e).contains("asleep") => {
                    runner
                        .client()
                        .power_on_stm32_domain(&target.link_uri)
                        .await
                        .context(CliError::Connection(format!("waking up {}", target.link_uri)))?;
                    runner.attach(target).await?
                }
                attached => attached?,
            };
            let platform = attached.cf.platform.device_type_name().map(str::to_string);
            attached.disconnect().await;
            platform.ok_or_else(|| anyhow!("its platform is unknown"))
        })
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

    // What each Crazyflie gets.
    let started = Instant::now();
    let total = runner.targets.len();
    let mut results: Vec<Option<Result<String>>> = std::iter::repeat_with(|| None).take(total).collect();
    let mut shown_platforms = vec![String::new(); total];
    let mut jobs = Vec::new();
    for (i, platform) in platforms.into_iter().enumerate() {
        let platform = match platform {
            Ok(platform) => platform,
            Err(e) => {
                results[i] = Some(Err(e));
                continue;
            }
        };
        shown_platforms[i] = platform.clone();
        match upgrades.iter().find(|(p, _)| *p == platform) {
            // Left out by --platform.
            None => results[i] = Some(Ok("skipped".to_string())),
            Some((_, upgrade)) if upgrade.get_target_and_types().is_empty() => {
                results[i] = Some(Ok(format!("nothing to flash for {}", platform)));
            }
            Some((_, upgrade)) => jobs.push(Job { index: i, platform, upgrade }),
        }
    }

    if !runner.is_direct() {
        for (i, result) in flash_through_server(runner, &jobs).await? {
            results[i] = Some(result);
        }
    } else if params.unicast {
        // One Crazyflie after another.
        let link_context = runner.link_context();
        for job in &jobs {
            let target = &runner.targets[job.index];
            println!();
            println!("{} ({} of {}, {}): {}", target.name, job.index + 1, total, job.platform, target.uri);
            let flashed =
                bootloader::flash(link_context, &target.link_uri, toc_cache.clone(), job.upgrade.clone(), false).await;
            results[job.index] = Some(flashed.map(|()| "flashed".to_string()));
        }
    } else {
        for (i, result) in broadcast::flash(runner, &jobs, toc_cache).await {
            results[i] = Some(result);
        }
    }
    let results: Vec<Result<String>> =
        results.into_iter().map(|result| result.expect("every Crazyflie has a result")).collect();

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
    let elapsed = started.elapsed().as_secs();
    println!("Took {}:{:02}", elapsed / 60, elapsed % 60);
    let (_, outcome) = split(results);
    outcome.finish()
}

/// Flash the jobs through the connection server: a job on the server per
/// platform, which broadcasts to the Crazyflies whose bootloaders take it and
/// flashes the others one after another. Returns a result for each job, by
/// target index. Fails before anything is flashed if a platform's firmware
/// can't go through a server.
async fn flash_through_server(runner: &Runner<'_>, jobs: &[Job<'_>]) -> Result<Vec<(usize, Result<String>)>> {
    let mut platforms: Vec<(&str, Vec<crazyflie_connection_server::FlashImage>, Vec<&Job>)> = Vec::new();
    for job in jobs {
        match platforms.iter_mut().find(|(platform, _, _)| *platform == job.platform) {
            Some((_, _, members)) => members.push(job),
            None => platforms.push((&job.platform, bootloader::server_images(job.upgrade)?, vec![job])),
        }
    }

    let mut results = Vec::new();
    for (platform, images, members) in platforms {
        bootloader::print_decks_skipped(members[0].upgrade);
        let (by_radio, by_usb): (Vec<&Job>, Vec<&Job>) =
            members.into_iter().partition(|job| runner.targets[job.index].is_radio());
        for job in by_usb {
            results.push((job.index, Err(anyhow!("stm32-fw / nrf51-fw flashing requires a radio URI"))));
        }
        if by_radio.is_empty() {
            continue;
        }
        if images.is_empty() {
            for job in by_radio {
                results.push((job.index, Ok("nothing to flash through a connection server".to_string())));
            }
            continue;
        }

        println!();
        println!("{} ({}) through the connection server", super::crazyflies(by_radio.len()), platform);
        let uris: Vec<&str> = by_radio.iter().map(|job| runner.targets[job.index].link_uri.as_str()).collect();
        let names: HashMap<String, &str> = by_radio
            .iter()
            .filter_map(|job| {
                let target = &runner.targets[job.index];
                Some((crazyflie_connection_server::normalize_uri(&target.link_uri).ok()?.key, target.name.as_str()))
            })
            .collect();
        let mut shown = JobProgress { names, bar: None };
        let flashed = runner
            .client()
            .flash_swarm(&uris, images, |progress| shown.show(progress))
            .await
            .context(CliError::Connection("flashing through the connection server".to_string()));
        shown.finish();
        for (job, result) in by_radio.into_iter().zip(flashed?) {
            results.push((job.index, result.map(|how| format!("flashed ({})", how)).map_err(|reason| anyhow!(reason))));
        }
    }
    Ok(results)
}

/// The progress of a flash job on a server: each step of each Crazyflie on
/// a line, a bar while an image is written.
struct JobProgress<'n> {
    /// Each Crazyflie's name, by normalized URI.
    names: HashMap<String, &'n str>,
    /// The bar shown, with the Crazyflie and step it is for.
    bar: Option<(Option<String>, String, indicatif::ProgressBar)>,
}

impl JobProgress<'_> {
    fn show(&mut self, progress: FlashProgress) {
        let label = match &progress.uri {
            Some(uri) => format!("{}: {}", self.names.get(uri).copied().unwrap_or(uri), progress.step),
            None => progress.step.clone(),
        };
        // Steps without a byte count report 0 or 1 of 1.
        if progress.total <= 1 {
            self.finish();
            println!("{}", label);
            return;
        }
        if self.bar.as_ref().is_none_or(|(uri, step, _)| *uri != progress.uri || *step != progress.step) {
            self.finish();
            let bar = get_progressbar(progress.total as usize, Some(&label));
            self.bar = Some((progress.uri.clone(), progress.step.clone(), bar));
        }
        if let Some((_, _, bar)) = &self.bar {
            bar.set_position(progress.done);
        }
    }

    fn finish(&mut self) {
        if let Some((uri, step, bar)) = self.bar.take() {
            let who = uri.as_ref().map(|uri| format!("{}: ", self.names.get(uri).copied().unwrap_or(uri)));
            finish_progress(&bar, format!("{}{} done", who.unwrap_or_default(), step));
        }
    }
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
