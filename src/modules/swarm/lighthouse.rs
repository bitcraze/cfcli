//! The lighthouse configuration of a swarm: which one it flies in (`swarm
//! config lighthouse`), and checking and writing it on its Crazyflies
//! (`swarm lh check|write`).
//!
//! A swarm names a stored lighthouse configuration (`lighthouse:` in the
//! swarm file). A shared swarm names a shared one, so that everyone who uses
//! the swarm gets it. Writing skips the Crazyflies that already have it,
//! which makes `swarm lh write` the way to give new Crazyflies the swarm's
//! configuration, and to update all of them when it changes.

use anyhow::{bail, Result};
use tabled::Tabled;

use super::runner::{csv_row_for, split, Runner, SwarmRow};
use super::Swarms;
use crate::error::CliError;
use crate::modules::documents::SharedId;
use crate::modules::lighthouse::configs::LhConfigs;
use crate::modules::lighthouse::{
    check_supported, compare, describe_distance, is_same, read_config, supported_base_stations, write_config,
    BaseStationDiff, CalibrationDelta, LighthouseConfigFile, Part,
};
use crate::utils::display::{csv_row, print_table, table};
use crate::{Config, SwarmLhCommands};

/// `swarm config lighthouse [CONFIG] [--clear]`
pub async fn link(swarms: &Swarms, config: &Config, id: &str, new: Option<&str>, clear: bool) -> Result<()> {
    if !clear && new.is_none() {
        let swarm = swarms.load(id).await?;
        match swarm.lighthouse {
            Some(lighthouse) => println!("Swarm '{}' flies in lighthouse config '{}'", id, lighthouse),
            None => println!(
                "Swarm '{}' names no lighthouse config; set one with 'cfcli swarm config lighthouse <CONFIG>'",
                id
            ),
        }
        return Ok(());
    }
    if let Some(new) = new {
        if SharedId::parse(id)?.is_some() && SharedId::parse(new)?.is_none() {
            bail!(CliError::InvalidValue(format!(
                "'{}' is shared, so it can only name a shared lighthouse config (<org>/<config>), \
                 not '{}' on this computer; share it with 'cfcli lh config move'",
                id, new
            )));
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
            "lighthouse config of swarm '{}'; name one with 'cfcli swarm config lighthouse <CONFIG>' or give --config",
            swarm_id
        )));
    };
    let configs = LhConfigs::open(config)?;
    let wanted = configs.load(id).await?;
    if !csv {
        let revision = SharedId::parse(id)?
            .and_then(|shared| configs.shared(&shared).ok()?.copy_state(&shared).ok()?)
            .map(|copy| format!(", revision {}", copy.revision))
            .unwrap_or_default();
        println!(
            "Lighthouse config '{}'{}: base stations {}",
            id,
            revision,
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
            "{} of {} have another lighthouse configuration; 'cfcli swarm lh write' gives them this one",
            differing,
            super::crazyflies(done.len())
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
            let back = read_config(cf, |_, _| {}).await?;
            if !is_same(&compare(wanted, &back)) {
                bail!("the Crazyflie has another configuration after writing it");
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
