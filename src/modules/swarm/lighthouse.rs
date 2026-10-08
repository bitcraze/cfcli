//! The lighthouse configuration of a swarm: which one it flies in (`swarm
//! config lh`), and checking and writing it on its Crazyflies
//! (`swarm lh check|write`).
//!
//! A swarm names a stored lighthouse configuration (`lighthouse:` in the
//! swarm file). A shared swarm names a shared one, so that everyone who uses
//! the swarm gets it. Writing skips the Crazyflies that already have it,
//! which makes `swarm lh write` the way to give new Crazyflies the swarm's
//! configuration, and to update all of them when it changes.

use anyhow::{anyhow, bail, Result};
use tabled::Tabled;

use super::runner::{csv_row_for, split, Runner, SwarmRow};
use super::store::Swarm;
use super::Swarms;
use crate::error::CliError;
use crate::modules::documents::{Entry, SharedId};
use crate::modules::lighthouse::configs::{describe, may_name, pick_entry, LhConfigs};
use crate::modules::lighthouse::{
    check_supported, compare, describe_distance, is_same, read_config, supported_base_stations, write_config,
    BaseStationDiff, CalibrationDelta, LighthouseConfigFile, Part,
};
use crate::utils::display::{csv_row, print_table, table};
use crate::{Config, SwarmLhCommands};

/// `swarm config lh [CONFIG] [--clear]`. Without a configuration, the user
/// picks one, or, when not interactive, it shows the one the swarm names.
pub async fn link(
    swarms: &Swarms,
    config: &Config,
    id: &str,
    new: Option<&str>,
    clear: bool,
    non_interactive: bool,
) -> Result<()> {
    let picked;
    let new = match new {
        Some(new) => Some(new),
        None if clear => None,
        None => {
            let current = swarms.load(id).await?.lighthouse;
            if non_interactive {
                match current {
                    Some(lighthouse) => println!("Swarm '{}' flies in lighthouse config '{}'", id, lighthouse),
                    None => println!(
                        "Swarm '{}' names no lighthouse config; set one with 'cfcli swarm config lh <CONFIG>'",
                        id
                    ),
                }
                return Ok(());
            }
            picked = pick_for(config, id, current.as_deref()).await?;
            Some(picked.as_str())
        }
    };
    if let Some(new) = new {
        if !may_name(id, new) {
            bail!(CliError::InvalidValue(match (SharedId::parse(id)?, SharedId::parse(new)?) {
                (None, _) => format!(
                    "'{}' is a swarm on this computer, so it can only name a lighthouse config on this \
                     computer, not '{}'; share the swarm with 'cfcli swarm config move {} <org>/{}' to fly in \
                     a shared one",
                    id, new, id, id
                ),
                (Some(swarm), None) => format!(
                    "'{}' is shared in {}, so it can only name a lighthouse config in {} ({}/<config>), not \
                     '{}' on this computer; share it with 'cfcli lh config move {} {}/{}'",
                    id, swarm.org, swarm.org, swarm.org, new, new, swarm.org, new
                ),
                (Some(swarm), Some(_)) => format!(
                    "'{}' is shared in {}, so it can only name a lighthouse config in {} ({}/<config>), not '{}'",
                    id, swarm.org, swarm.org, swarm.org, new
                ),
            }));
        }
        // It must exist (and a shared one gets a copy here).
        LhConfigs::open(config)?.load(new).await?;
    }
    let new = new.map(str::to_string);
    swarms
        .change(id, |swarm| {
            swarm.lighthouse = new.clone();
            Ok(())
        })
        .await?;
    match &new {
        Some(new) => println!("Swarm '{}' flies in lighthouse config '{}'", id, new),
        None => println!("Swarm '{}' names no lighthouse config now", id),
    }
    Ok(())
}

/// Let the user pick the configuration swarm `id` flies in: one on this
/// computer for a local swarm, one in its organization for a shared one.
async fn pick_for(config: &Config, id: &str, current: Option<&str>) -> Result<String> {
    let org = SharedId::parse(id)?.map(|shared| shared.org);
    let entries: Vec<Entry> = LhConfigs::open(config)?
        .entries()
        .await?
        .into_iter()
        .filter(|entry| may_name(id, &entry.id))
        .collect();
    if entries.is_empty() {
        bail!(CliError::NotFound(match &org {
            None => "lighthouse configs on this computer; store one with 'cfcli lh config save <config>' or \
                     'cfcli lh config import <file>'"
                .to_string(),
            Some(org) => format!(
                "lighthouse configs in {}; store one with 'cfcli lh config save {}/<config>', or share one with \
                 'cfcli lh config move <config> {}/<config>'",
                org, org, org
            ),
        }));
    }
    let message = match current {
        Some(current) => format!("Lighthouse config for swarm '{}' (now '{}'):", id, current),
        None => format!("Lighthouse config for swarm '{}':", id),
    };
    pick_entry(&entries, &message, current)
}

/// `swarm lh check|write`
pub async fn run(
    config: &Config,
    runner: &Runner<'_>,
    swarm_id: &str,
    linked: Option<&str>,
    command: &SwarmLhCommands,
    csv: bool,
) -> Result<()> {
    let given = match command {
        SwarmLhCommands::Check(params) => params.config.as_deref(),
        SwarmLhCommands::Write(params) => params.config.as_deref(),
    };
    let Some(id) = given.or(linked) else {
        bail!(CliError::NotFound(format!(
            "lighthouse config of swarm '{}'; name one with 'cfcli swarm config lh <CONFIG>' or give --config",
            swarm_id
        )));
    };
    let configs = LhConfigs::open(config)?;
    let wanted = configs.load(id).await?;
    if !csv {
        println!(
            "Lighthouse config {}: base stations {}",
            describe(&configs, id),
            ids(&wanted.geos.keys().copied().collect::<Vec<_>>())
        );
    }
    match command {
        SwarmLhCommands::Check(_) => check(runner, &wanted, csv).await,
        SwarmLhCommands::Write(params) => write(runner, &wanted, params.force).await,
    }
}

fn ids(ids: &[u8]) -> String {
    match ids {
        [] => "none".to_string(),
        ids => ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", "),
    }
}

/// What a check found on one Crazyflie.
struct Found {
    diffs: Vec<BaseStationDiff>,
    /// How many base stations its firmware supports, if that can be told.
    supported: Option<u8>,
}

impl Found {
    fn up_to_date(&self) -> bool {
        is_same(&self.diffs)
    }

    /// Base stations whose geometry or calibration differs.
    fn differing(&self) -> Vec<u8> {
        self.diffs.iter().filter(|d| !d.is_same()).map(|d| d.id).collect()
    }

    /// "BS 4, 5 not on the Crazyflie; BS 2 moved 3.1 cm; firmware supports 4 base stations"
    fn summary(&self, wanted: &LighthouseConfigFile) -> String {
        if self.up_to_date() {
            return "up to date".to_string();
        }
        let select = |f: &dyn Fn(&BaseStationDiff) -> bool| {
            self.diffs.iter().filter(|d| f(d)).map(|d| d.id).collect::<Vec<u8>>()
        };
        let mut parts = Vec::new();
        let missing = select(&|d| matches!(d.geometry, Part::OnlyInFile));
        if !missing.is_empty() {
            parts.push(format!("no position for BS {}", ids(&missing)));
        }
        for d in &self.diffs {
            if let Part::Differs(delta) = d.geometry {
                parts.push(format!(
                    "BS {} moved {}, turned {:.2}°",
                    d.id,
                    describe_distance(delta.moved_m),
                    delta.turned_deg
                ));
            }
        }
        let extra = select(&|d| matches!(d.geometry, Part::OnlyOnCf));
        if !extra.is_empty() {
            parts.push(format!("position for BS {} that the config hasn't", ids(&extra)));
        }
        let replaced = select(&|d| matches!(d.calibration, Part::Differs(CalibrationDelta::Replaced { .. })));
        if !replaced.is_empty() {
            parts.push(format!("another base station on BS {}", ids(&replaced)));
        }
        let calibration = select(&|d| {
            matches!(
                d.calibration,
                Part::Differs(CalibrationDelta::Values { .. }) | Part::OnlyInFile | Part::OnlyOnCf
            )
        });
        if !calibration.is_empty() {
            parts.push(format!("calibration differs for BS {}", ids(&calibration)));
        }
        if let Some(supported) = self.supported {
            if wanted.ids().iter().any(|id| *id >= supported) {
                parts.push(format!("its firmware supports only BS 0-{}", supported.saturating_sub(1)));
            }
        }
        parts.join("; ")
    }
}

/// One row of `swarm lh check`.
#[derive(Tabled)]
struct CheckRow {
    #[tabled(rename = "Lighthouse")]
    status: String,
}

async fn check(runner: &Runner<'_>, wanted: &LighthouseConfigFile, csv: bool) -> Result<()> {
    let results = runner
        .connected("Reading", async |cf, _| {
            let on_cf = read_config(cf, |_, _| {}).await?;
            Ok(Found { diffs: compare(wanted, &on_cf), supported: supported_base_stations(cf) })
        })
        .await;
    let (done, outcome) = split(results);
    let differing = done.iter().filter(|(_, found)| !found.up_to_date()).count();
    if csv {
        csv_row(&["cf", "uri", "status", "firmware_base_stations", "differing_base_stations"]);
        for (i, found) in &done {
            let differing: Vec<String> = found.differing().iter().map(|id| id.to_string()).collect();
            csv_row_for(
                &runner.targets[*i],
                &[
                    if found.up_to_date() { "up_to_date" } else { "differs" },
                    &found.supported.map(|n| n.to_string()).unwrap_or_default(),
                    &differing.join(";"),
                ],
            );
        }
    } else {
        let rows: Vec<SwarmRow<CheckRow>> = done
            .iter()
            .map(|(i, found)| SwarmRow {
                cf: runner.targets[*i].name.clone(),
                row: CheckRow { status: found.summary(wanted) },
            })
            .collect();
        if !rows.is_empty() {
            print_table(&table(&rows));
        }
    }
    outcome.print_failures(&runner.targets);
    outcome.finish()?;
    if differing > 0 {
        bail!(CliError::Differs(format!(
            "{} of {} {} another lighthouse configuration; 'cfcli swarm lh write' gives them this one",
            differing,
            super::crazyflies(done.len()),
            if differing == 1 { "has" } else { "have" }
        )));
    }
    Ok(())
}

async fn write(runner: &Runner<'_>, wanted: &LighthouseConfigFile, force: bool) -> Result<()> {
    let results = runner
        .connected("Writing", async |cf, _| {
            // Refused before anything is written.
            check_supported(wanted, supported_base_stations(cf))?;
            if !force && is_same(&compare(wanted, &read_config(cf, |_, _| {}).await?)) {
                return Ok(false);
            }
            write_config(cf, wanted, |_, _| {}).await?;
            let diffs = compare(wanted, &read_config(cf, |_, _| {}).await?);
            if !is_same(&diffs) {
                return Err(not_kept(wanted, diffs));
            }
            Ok(true)
        })
        .await;
    let (done, outcome) = split(results);
    for (i, target) in runner.targets.iter().enumerate() {
        if let Some((_, written)) = done.iter().find(|(d, _)| *d == i) {
            println!(
                "{}: {}",
                target.name,
                if *written { "written and stored in flash" } else { "up to date" }
            );
        }
    }
    outcome.print_failures(&runner.targets);
    outcome.finish()
}

/// Why a Crazyflie doesn't have the configuration when it is read back
/// right after writing. Usually the Crazyflie has taken the calibration of
/// a base station it sees whose UID isn't the configuration's (see
/// [`CalibrationDelta::Replaced`]).
fn not_kept(wanted: &LighthouseConfigFile, diffs: Vec<BaseStationDiff>) -> anyhow::Error {
    let replaced: Vec<String> = diffs
        .iter()
        .filter_map(|d| match d.calibration {
            Part::Differs(CalibrationDelta::Replaced { file_uid, cf_uid }) => Some(format!(
                "BS {} sees 0x{:08X}, the config has 0x{:08X}",
                d.id, cf_uid, file_uid
            )),
            _ => None,
        })
        .collect();
    let only_replaced = diffs.iter().all(|d| {
        matches!(d.geometry, Part::Same | Part::Absent)
            && matches!(
                d.calibration,
                Part::Same | Part::Absent | Part::Differs(CalibrationDelta::Replaced { .. })
            )
    });
    if !replaced.is_empty() && only_replaced {
        return anyhow!(
            "written, but then the Crazyflie took the calibration of the base stations it sees, which aren't the \
             config's ({}). If a base station was replaced, its geometry may need a new estimate; then store \
             the configuration again with 'cfcli lh config save'",
            replaced.join("; ")
        );
    }
    let found = Found { diffs, supported: None };
    anyhow!("the Crazyflie has another configuration after writing it: {}", found.summary(wanted))
}

/// A hint after Crazyflies were added to a swarm that names a lighthouse
/// configuration: how to give it to them.
pub fn hint_for_new(swarm: &str, lighthouse: Option<&str>, added: &[String]) {
    if let (Some(lighthouse), false) = (lighthouse, added.is_empty()) {
        println!(
            "The swarm flies in lighthouse config '{}'; give it to the new Crazyflies with \
             'cfcli swarm lh write --swarm {} --cf {}'",
            lighthouse,
            swarm,
            added.join(",")
        );
    }
}

/// When a swarm becomes swarm `id` (moved or imported), drop the lighthouse
/// configuration it names if `id` can't name it (see [`may_name`]): the
/// server refuses a shared swarm naming another organization's. Returns what
/// to tell the user.
pub fn drop_unnameable(id: &str, swarm: &mut Swarm) -> Option<String> {
    let dropped = swarm.lighthouse.take_if(|lighthouse| !may_name(id, lighthouse))?;
    Some(format!(
        "Swarm '{}' names no lighthouse config: it can't name '{}' (a local swarm names lighthouse configs on \
         this computer, a shared swarm those in its organization); name one with 'cfcli swarm config lh \
         <CONFIG> --swarm {}'",
        id, dropped, id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_what_the_new_id_cannot_name() {
        let swarm = |lighthouse: &str| Swarm {
            lighthouse: Some(lighthouse.to_string()),
            ..Swarm::new("s".to_string(), None)
        };
        for (id, lighthouse, kept) in [
            ("lab/s", "lab/cage", true),
            ("other/s", "lab/cage", false),
            ("s", "lab/cage", false),
            ("lab/s", "cage", false),
            ("s", "cage", true),
        ] {
            let mut moved = swarm(lighthouse);
            let note = drop_unnameable(id, &mut moved);
            assert_eq!(moved.lighthouse.is_some(), kept, "{} naming {}", id, lighthouse);
            assert_eq!(note.is_none(), kept, "{} naming {}", id, lighthouse);
        }
        let mut none = Swarm::new("s".to_string(), None);
        assert!(drop_unnameable("lab/s", &mut none).is_none());
    }

    const FILE: &str = "\
type: lighthouse_system_configuration
version: '1'
systemType: 2
geos:
  0:
    origin: [0.0, 0.0, 2.0]
    rotation: [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
calibs:
  0:
    uid: 2360210604
    sweeps:
    - {phase: 0.0, tilt: -0.051, curve: 0.275, gibmag: -0.005, gibphase: 2.281, ogeemag: -0.184, ogeephase: 1.847}
    - {phase: -0.004, tilt: 0.047, curve: 0.367, gibmag: -0.005, gibphase: 2.548, ogeemag: -0.124, ogeephase: 2.051}
";

    #[test]
    fn says_why_a_write_did_not_stay() {
        let wanted = LighthouseConfigFile::from_yaml(FILE).unwrap();

        // The Crazyflie took the calibration of the base station it sees.
        let mut seen = wanted.clone();
        seen.calibs.get_mut(&0).unwrap().uid = 0x12345678;
        let message = not_kept(&wanted, compare(&wanted, &seen)).to_string();
        assert!(message.contains("BS 0 sees 0x12345678, the config has 0x8CADF4AC"), "{}", message);

        // Anything else: what differs.
        let mut moved = seen.clone();
        moved.geos.get_mut(&0).unwrap().origin[0] = 0.05;
        let message = not_kept(&wanted, compare(&wanted, &moved)).to_string();
        assert!(message.starts_with("the Crazyflie has another configuration"), "{}", message);
        assert!(message.contains("BS 0 moved 5.0 cm"), "{}", message);
    }
}
