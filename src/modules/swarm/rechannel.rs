//! `cfcli swarm rechannel`: spread a swarm over several radio channels.
//!
//! With one Crazyradio per channel, each radio then serves its own part of
//! the swarm (Crazyflies on one channel always share a radio, see
//! [`crate::utils::radio::assign`]).
//!
//! The channel is stored in each Crazyflie's EEPROM config and the firmware
//! only applies it at boot, so a Crazyflie that moves is reprogrammed on its
//! current channel, rebooted and then looked for on its new channel. Its URI
//! in the swarm file only changes once it answers there. Running the command
//! again therefore finishes a run that was interrupted: a Crazyflie that no
//! longer answers on its old channel but does on its new one only gets its
//! URI updated.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use crazyflie_lib::crazyflie_link::LinkContext;
use inquire::Confirm;
use tabled::Tabled;

use super::runner::{split, Runner, Target};
use super::store::Store;
use crate::error::CliError;
use crate::modules::{bootloader, config};
use crate::utils::display::{print_table, table};
use crate::utils::radio::RadioUri;
use crate::{Config, ConfigTocCache, SwarmRechannelParameters};

/// The lower channels are the more crowded ones (Wi-Fi), so `--count`
/// starts here and goes down.
const FIRST_CHANNEL: u8 = 80;
/// Channels less than 2 apart interfere at 2 Mbit/s.
const CHANNEL_STEP: u8 = 2;
/// How long a rebooted Crazyflie gets to show up on its new channel.
const BOOT_TIMEOUT: Duration = Duration::from_secs(8);
/// Pause between looking for a rebooting Crazyflie.
const BOOT_POLL: Duration = Duration::from_millis(300);

/// The channels `--count` uses: 80, 78, 76, ...
pub fn channels_for_count(count: u8) -> Vec<u8> {
    (0..count).map(|i| FIRST_CHANNEL - CHANNEL_STEP * i).collect()
}

/// The new channel of each Crazyflie, given their current channels.
///
/// Each channel gets an equal share, the shares differ by at most one and
/// the first channels get the extra. Crazyflies already on one of the
/// channels stay there, up to that channel's share, in list order; the rest
/// fill the remaining places, in list order. That keeps the number of
/// Crazyflies to reprogram as low as possible, and planning again from the
/// result changes nothing.
pub fn plan(current: &[u8], channels: &[u8]) -> Vec<u8> {
    let (n, k) = (current.len(), channels.len());
    let share: Vec<usize> = (0..k).map(|j| n / k + usize::from(j < n % k)).collect();
    let mut filled = vec![0; k];
    let mut assigned: Vec<Option<usize>> = vec![None; n];

    for (i, channel) in current.iter().enumerate() {
        if let Some(j) = channels.iter().position(|c| c == channel) {
            if filled[j] < share[j] {
                filled[j] += 1;
                assigned[i] = Some(j);
            }
        }
    }
    for slot in assigned.iter_mut().filter(|slot| slot.is_none()) {
        let j = (0..k).find(|j| filled[*j] < share[*j]).expect("the shares add up to the Crazyflies");
        filled[j] += 1;
        *slot = Some(j);
    }
    assigned.into_iter().map(|j| channels[j.expect("every Crazyflie has a channel")]).collect()
}

/// One Crazyflie that changes channel.
struct Move {
    /// Its index in the swarm.
    unit: usize,
    name: String,
    from: u8,
    to: u8,
    old_uri: String,
    new_uri: String,
}

/// One row of the plan.
#[derive(Tabled)]
struct PlanRow {
    #[tabled(rename = "CF")]
    name: String,
    #[tabled(rename = "URI")]
    uri: String,
    #[tabled(rename = "From")]
    from: u8,
    #[tabled(rename = "To")]
    to: u8,
}

/// One row of the summary.
#[derive(Tabled)]
struct ResultRow {
    #[tabled(rename = "CF")]
    name: String,
    #[tabled(rename = "From")]
    from: u8,
    #[tabled(rename = "To")]
    to: u8,
    #[tabled(rename = "Result")]
    result: String,
}

pub(super) async fn rechannel(
    store: &Store,
    config: &mut Config,
    params: &SwarmRechannelParameters,
    link_context: &LinkContext,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
) -> Result<()> {
    let target = &params.target;
    let id = super::swarm_id(config, target.swarm.as_deref())?;
    let mut swarm = store.load(&id)?;
    let selected = swarm.select(&id, &target.cf, &target.exclude)?;

    let channels = match params.count {
        Some(count) => channels_for_count(count),
        None => params.channels.clone(),
    };
    check_channels(&channels)?;

    // Only Crazyflies on the radio have a channel to change.
    let mut radio_units = Vec::new();
    for i in selected {
        match RadioUri::parse(&swarm.units[i].uri)? {
            Some(uri) => radio_units.push((i, uri)),
            None => eprintln!("Skipping {}: it isn't reached over the radio", swarm.units[i].name),
        }
    }
    if radio_units.is_empty() {
        bail!(CliError::InvalidValue("none of the Crazyflies is reached over the radio".to_string()));
    }
    if channels.len() > radio_units.len() {
        eprintln!("Warning: more channels than Crazyflies, some channels stay empty");
    }

    let current: Vec<u8> = radio_units.iter().map(|(_, uri)| uri.channel).collect();
    let moves: Vec<Move> = radio_units
        .iter()
        .zip(plan(&current, &channels))
        .filter(|((_, uri), to)| uri.channel != *to)
        .map(|((i, uri), to)| Move {
            unit: *i,
            name: swarm.units[*i].name.clone(),
            from: uri.channel,
            to,
            old_uri: swarm.units[*i].uri.clone(),
            new_uri: uri.with_channel(to),
        })
        .collect();
    check_addresses(&swarm, &moves)?;

    if moves.is_empty() {
        println!("Nothing to do: every Crazyflie is already on its channel");
        return Ok(());
    }
    let rows: Vec<PlanRow> = moves
        .iter()
        .map(|m| PlanRow { name: m.name.clone(), uri: m.old_uri.clone(), from: m.from, to: m.to })
        .collect();
    print_table(&table(&rows));
    println!("{} of {} move", moves.len(), super::crazyflies(radio_units.len()));
    if params.dry_run {
        return Ok(());
    }
    if !params.yes {
        crate::require_arg(non_interactive, "--yes")?;
        let question = format!("Reprogram {}?", super::crazyflies(moves.len()));
        if !Confirm::new(&question).with_default(false).prompt().unwrap_or(false) {
            println!("Nothing changed");
            return Ok(());
        }
    }

    // The same Crazyflies with their old and their new URIs, so that each
    // step talks to them on the right channel through the right radio.
    let mut moved = swarm.clone();
    for m in &moves {
        moved.units[m.unit].uri = m.new_uri.clone();
    }
    let units: Vec<usize> = moves.iter().map(|m| m.unit).collect();
    let on_old = Runner::new(link_context, config, toc_cache.clone(), &swarm, &units).await?;
    let on_new = Runner::new(link_context, config, toc_cache.clone(), &moved, &units).await?;
    let index_of = |target: &Target| moves.iter().position(|m| m.name == target.name).expect("a moving Crazyflie");

    // 1. Where each one is now.
    let answers_old = answering(&on_old, "Checking", link_context).await;
    let answers_new = answering(&on_new, "Checking", link_context).await;

    // 2. Reprogram the ones on their old channel.
    let mut results: Vec<Option<Result<String>>> = std::iter::repeat_with(|| None).take(moves.len()).collect();
    let to_write: Vec<usize> = (0..moves.len()).filter(|i| answers_old[*i]).collect();
    let mut rebooted = Vec::new();
    if !to_write.is_empty() {
        let writer = runner_for(link_context, config, &toc_cache, &swarm, &moves, &to_write).await?;
        let written = writer
            .connected("Reprogramming", async |cf, target| {
                config::write_radio_channel(cf, moves[index_of(target)].to).await
            })
            .await;
        let mut done = Vec::new();
        for (k, result) in written.into_iter().enumerate() {
            match result {
                Ok(()) => done.push(to_write[k]),
                Err(e) => results[to_write[k]] = Some(Err(e.context("not reprogrammed"))),
            }
        }
        // 3. Reboot them, so that the firmware picks up the new channel.
        if !done.is_empty() {
            let rebooter = runner_for(link_context, config, &toc_cache, &swarm, &moves, &done).await?;
            let reboots = rebooter
                .each("Rebooting", async |t| bootloader::reboot(link_context, &t.link_uri).await)
                .await;
            for (k, result) in reboots.into_iter().enumerate() {
                match result {
                    Ok(()) => rebooted.push(done[k]),
                    Err(e) => results[done[k]] = Some(Err(e.context("reprogrammed but not rebooted"))),
                }
            }
        }
    }

    // 4. Look for the rebooted ones on their new channel.
    let mut found = vec![false; moves.len()];
    if !rebooted.is_empty() {
        let finder = runner_for(link_context, config, &toc_cache, &moved, &moves, &rebooted).await?;
        let looked = finder
            .each("Looking on the new channels", async |t| Ok(wait_for(t, link_context).await))
            .await;
        for (k, result) in looked.into_iter().enumerate() {
            found[rebooted[k]] = result.unwrap_or(false);
        }
    }
    let rebooted_not_found: Vec<usize> = rebooted.iter().copied().filter(|i| !found[*i]).collect();
    let mut still_on_old = vec![false; moves.len()];
    if !rebooted_not_found.is_empty() {
        let checker = runner_for(link_context, config, &toc_cache, &swarm, &moves, &rebooted_not_found).await?;
        for (k, answers) in answering(&checker, "Checking", link_context).await.into_iter().enumerate() {
            still_on_old[rebooted_not_found[k]] = answers;
        }
    }

    for (i, m) in moves.iter().enumerate() {
        if results[i].is_some() {
            continue;
        }
        results[i] = Some(if found[i] {
            Ok("moved".to_string())
        } else if rebooted.contains(&i) && still_on_old[i] {
            Err(anyhow!("still on channel {} after the reboot, the new channel didn't take", m.from))
        } else if rebooted.contains(&i) {
            Err(anyhow!(CliError::Connection(format!(
                "doesn't answer on channel {} or {} after the reboot",
                m.to, m.from
            ))))
        } else if answers_new[i] {
            // From a run that stopped after the reboot.
            found[i] = true;
            Ok("already on the new channel".to_string())
        } else {
            Err(anyhow!(CliError::Connection(format!("doesn't answer on channel {} or {}", m.from, m.to))))
        });
    }

    // 5. The swarm file (and the selected URI) follow the Crazyflies that
    // answer on their new channel.
    let mut changed = false;
    for (i, m) in moves.iter().enumerate() {
        if found[i] {
            swarm.units[m.unit].uri = m.new_uri.clone();
            if let Some(uri) = follow_move(&config.uri, &m.old_uri, m.to) {
                config.uri = uri;
                changed = true;
            }
        }
    }
    if found.iter().any(|f| *f) {
        store.save(&id, &swarm)?;
    }
    if changed {
        super::save_config(config);
    }

    let results: Vec<Result<String>> = results.into_iter().map(|r| r.expect("every move has a result")).collect();
    let rows: Vec<ResultRow> = moves
        .iter()
        .zip(&results)
        .map(|(m, result)| ResultRow {
            name: m.name.clone(),
            from: m.from,
            to: m.to,
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

/// The selected URI after a Crazyflie moved from `old_uri` to channel `to`,
/// if the selected URI is that Crazyflie: the same channel and address,
/// however its radio is written (`cfcli select` writes `radio://0/`). Its
/// radio and options are kept.
fn follow_move(selected: &str, old_uri: &str, to: u8) -> Option<String> {
    let selected = RadioUri::parse(selected).ok()??;
    let old = RadioUri::parse(old_uri).ok()??;
    (selected.channel == old.channel && selected.address == old.address).then(|| selected.with_channel(to))
}

/// A runner for some of the moving Crazyflies (indices into `moves`).
async fn runner_for<'a>(
    link_context: &'a LinkContext,
    config: &Config,
    toc_cache: &ConfigTocCache,
    swarm: &super::store::Swarm,
    moves: &[Move],
    which: &[usize],
) -> Result<Runner<'a>> {
    let units: Vec<usize> = which.iter().map(|i| moves[*i].unit).collect();
    Runner::new(link_context, config, toc_cache.clone(), swarm, &units).await
}

/// Whether each of the runner's Crazyflies answers.
async fn answering(runner: &Runner<'_>, label: &str, link_context: &LinkContext) -> Vec<bool> {
    runner
        .each(label, async |t| Ok(t.answers(link_context).await))
        .await
        .into_iter()
        .map(|answers| answers.unwrap_or(false))
        .collect()
}

/// Wait for a rebooted Crazyflie to answer.
async fn wait_for(target: &Target, link_context: &LinkContext) -> bool {
    let deadline = tokio::time::Instant::now() + BOOT_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if target.answers(link_context).await {
            return true;
        }
        tokio::time::sleep(BOOT_POLL).await;
    }
    false
}

/// Reject duplicate channels, warn about channels that interfere.
fn check_channels(channels: &[u8]) -> Result<()> {
    if channels.is_empty() {
        bail!(CliError::InvalidValue("no channels given".to_string()));
    }
    for (i, a) in channels.iter().enumerate() {
        for b in &channels[i + 1..] {
            if a == b {
                bail!(CliError::InvalidValue(format!("channel {} is given twice", a)));
            }
            if a.abs_diff(*b) < CHANNEL_STEP {
                eprintln!(
                    "Warning: channels {} and {} are less than {} apart; they interfere at 2M and share a Crazyradio",
                    a, b, CHANNEL_STEP
                );
            }
        }
    }
    Ok(())
}

/// Two Crazyflies on the same channel with the same address can't be told
/// apart. Check the swarm as it will be after the moves, the Crazyflies
/// that don't move included.
fn check_addresses(swarm: &super::store::Swarm, moves: &[Move]) -> Result<()> {
    let mut seen: HashMap<(u8, String), &str> = HashMap::new();
    for (i, unit) in swarm.units.iter().enumerate() {
        let Some(uri) = RadioUri::parse(&unit.uri)? else { continue };
        let channel = moves.iter().find(|m| m.unit == i).map_or(uri.channel, |m| m.to);
        if let Some(other) = seen.insert((channel, uri.address.clone()), &unit.name) {
            bail!(CliError::InvalidValue(format!(
                "{} and {} would both be on channel {} with address {}; give them different addresses first \
                 (cfcli config set address=...)",
                other, unit.name, channel, uri.address
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_starts_at_80_and_steps_down_by_2() {
        assert_eq!(channels_for_count(1), vec![80]);
        assert_eq!(channels_for_count(3), vec![80, 78, 76]);
        assert_eq!(*channels_for_count(41).last().unwrap(), 0);
    }

    #[test]
    fn splits_evenly_with_the_extra_on_the_first_channels() {
        let new = plan(&[90; 7], &[80, 78, 76]);
        assert_eq!(new, vec![80, 80, 80, 78, 78, 76, 76]);
    }

    #[test]
    fn crazyflies_already_on_a_channel_stay_up_to_its_share() {
        // Three on 78 already, but its share is 2: the first two stay.
        assert_eq!(plan(&[78, 90, 78, 78], &[80, 78]), vec![78, 80, 78, 80]);
    }

    #[test]
    fn moves_as_few_as_possible() {
        // 100 each on 72, 76 and 80 spread over 80, 78 and 76: only the ones
        // on 72 move, to 78.
        let mut current = vec![72; 100];
        current.extend([76; 100]);
        current.extend([80; 100]);
        let new = plan(&current, &channels_for_count(3));
        let moved = current.iter().zip(&new).filter(|(a, b)| a != b).count();
        assert_eq!(moved, 100);
        assert!(new[..100].iter().all(|c| *c == 78));
    }

    #[test]
    fn planning_again_changes_nothing() {
        let current = vec![90, 90, 80, 72, 72, 72, 76];
        let channels = channels_for_count(3);
        let new = plan(&current, &channels);
        assert_eq!(plan(&new, &channels), new);
    }

    #[test]
    fn more_channels_than_crazyflies() {
        assert_eq!(plan(&[90, 90], &[80, 78, 76]), vec![80, 78]);
    }

    #[test]
    fn the_selected_uri_follows_its_crazyflie() {
        let old = "radio:///80/2M/E7E7E7E701";
        assert_eq!(follow_move("radio://0/80/2M/E7E7E7E701", old, 90).as_deref(), Some("radio://0/90/2M/E7E7E7E701"));
        assert_eq!(follow_move(old, old, 90).as_deref(), Some("radio:///90/2M/E7E7E7E701"));
        assert_eq!(
            follow_move("radio://1/80/2M/e7e7e7e701?safelink=0", old, 90).as_deref(),
            Some("radio://1/90/2M/e7e7e7e701?safelink=0")
        );
        assert_eq!(follow_move("radio://0/80/2M/E7E7E7E702", old, 90), None);
        assert_eq!(follow_move("radio://0/81/2M/E7E7E7E701", old, 90), None);
        assert_eq!(follow_move("usb://0", old, 90), None);
    }

    #[test]
    fn rejects_duplicate_channels() {
        assert!(check_channels(&[80, 78, 80]).is_err());
        assert!(check_channels(&[]).is_err());
        assert!(check_channels(&[80, 79]).is_ok());
    }

    fn swarm(uris: &[&str]) -> super::super::store::Swarm {
        let mut swarm = super::super::store::Swarm::new("s".to_string(), None);
        for (i, uri) in uris.iter().enumerate() {
            swarm.units.push(super::super::store::Unit {
                uri: uri.to_string(),
                name: format!("CF-{:02}", i + 1),
                description: None,
                extra: Default::default(),
            });
        }
        swarm
    }

    fn moving(unit: usize, from: u8, to: u8) -> Move {
        Move { unit, name: String::new(), from, to, old_uri: String::new(), new_uri: String::new() }
    }

    #[test]
    fn refuses_two_crazyflies_on_one_channel_with_one_address() {
        let swarm = swarm(&["radio:///80/2M/E7E7E7E7E7", "radio:///90/2M/E7E7E7E7E7", "radio:///90/2M/E7E7E7E701"]);
        let err = check_addresses(&swarm, &[moving(1, 90, 80)]).unwrap_err();
        assert!(format!("{:#}", err).contains("CF-01 and CF-02 would both be on channel 80"), "{:#}", err);
        assert!(check_addresses(&swarm, &[moving(2, 90, 80)]).is_ok());
    }
}
