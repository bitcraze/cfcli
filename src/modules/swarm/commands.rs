//! The normal cfcli commands, run on every Crazyflie of a swarm.
//!
//! Listings look like the normal ones with the Crazyflie's name added in
//! front (and its name and URI in front of each CSV row). Commands that only
//! do something print one line per Crazyflie. Crazyflies that fail are
//! reported on stderr, see [`Outcome::finish`] for the exit code.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use crazyflie_lib::Crazyflie;
use tabled::Tabled;

use super::runner::{csv_row_for, split, Outcome, Runner, SwarmRow};
use crate::modules::{bootloader, debug, deck, param, platform};
use crate::utils::display::{csv_row, print_table, table};
use crate::{
    AssertArgs, SwarmDebugCommands, SwarmDeckCommands, SwarmParamCommands, SwarmPlatformCommands,
    VariableName, VariableNameAndValue,
};

pub async fn platform(runner: &Runner<'_>, command: &SwarmPlatformCommands, csv: bool) -> Result<()> {
    let link_context = runner.link_context();
    // These commands are sent without waiting for an answer, so each
    // Crazyflie is first checked to answer at all. Otherwise one that is
    // switched off would be reported as done. A sleeping Crazyflie answers:
    // its nRF51 stays on.
    let (results, done) = match command {
        SwarmPlatformCommands::Info => return platform_info(runner, csv).await,
        SwarmPlatformCommands::Reboot => (
            runner
                .each("Rebooting", async |t| {
                    t.check_answers(link_context).await?;
                    bootloader::reboot(link_context, &t.link_uri).await
                })
                .await,
            "rebooted",
        ),
        SwarmPlatformCommands::PowerOff => (
            runner
                .each("Powering off", async |t| {
                    t.check_answers(link_context).await?;
                    Ok(Crazyflie::power_off_all(link_context, &t.link_uri).await?)
                })
                .await,
            "powered off",
        ),
        SwarmPlatformCommands::Sleep => (
            runner
                .each("Putting to sleep", async |t| {
                    t.check_answers(link_context).await?;
                    Ok(Crazyflie::power_off_stm32_domain(link_context, &t.link_uri).await?)
                })
                .await,
            "asleep",
        ),
        SwarmPlatformCommands::Wakeup => (
            runner
                .each("Waking up", async |t| {
                    t.check_answers(link_context).await?;
                    Ok(Crazyflie::power_on_stm32_domain(link_context, &t.link_uri).await?)
                })
                .await,
            "woken up",
        ),
    };
    finish_actions(runner, results, done)
}

async fn platform_info(runner: &Runner<'_>, csv: bool) -> Result<()> {
    let results = runner.connected("Reading", async |cf| platform::info(cf).await).await;
    let (done, outcome) = split(results);
    if csv {
        let mut header = vec!["cf", "uri"];
        header.extend(platform::PlatformInfo::CSV_HEADER);
        csv_row(&header);
        for (i, info) in &done {
            csv_row_for(&runner.targets[*i], &info.csv_fields());
        }
    } else if !done.is_empty() {
        let rows: Vec<_> = done.into_iter().map(|(i, info)| row(runner, i, info)).collect();
        print_table(&table(&rows));
    }
    finish(runner, outcome)
}

pub async fn deck(runner: &Runner<'_>, command: &SwarmDeckCommands, csv: bool) -> Result<()> {
    let SwarmDeckCommands::List = command;
    let results = runner.connected("Reading", async |cf| Ok(deck::decks(cf).await)).await;
    let (done, outcome) = split(results);
    if csv {
        csv_row(&["cf", "uri", "name", "revision", "serial"]);
        for (i, decks) in &done {
            for d in decks {
                csv_row_for(&runner.targets[*i], &[&d.name, &d.revision, &d.serial]);
            }
        }
    } else if !done.is_empty() {
        let mut rows = Vec::new();
        for (i, decks) in done {
            // A Crazyflie without decks still gets a line, or it would look
            // like it failed.
            if decks.is_empty() {
                let none = deck::Deck { name: "(no decks)".to_string(), revision: String::new(), serial: String::new() };
                rows.push(row(runner, i, none));
            }
            rows.extend(decks.into_iter().map(|d| row(runner, i, d)));
        }
        print_table(&table(&rows));
    }
    finish(runner, outcome)
}

/// One row of `swarm debug assert`.
#[derive(Tabled)]
struct AssertRow {
    #[tabled(rename = "Assert info")]
    info: String,
}

pub async fn debug(runner: &Runner<'_>, command: &SwarmDebugCommands, csv: bool) -> Result<()> {
    let SwarmDebugCommands::Assert(AssertArgs { wait_timeout_ms }) = command;
    let wait = Duration::from_millis(*wait_timeout_ms);
    let results = runner.connected("Reading", async |cf| debug::assert_info(cf, wait).await).await;
    let (done, outcome) = split(results);
    if csv {
        csv_row(&["cf", "uri", "assert_info"]);
        for (i, info) in &done {
            csv_row_for(&runner.targets[*i], &[info.as_deref().unwrap_or_default()]);
        }
    } else if !done.is_empty() {
        let rows: Vec<_> = done
            .into_iter()
            .map(|(i, info)| row(runner, i, AssertRow { info: info.unwrap_or_else(|| "No assert info".to_string()) }))
            .collect();
        print_table(&table(&rows));
    }
    finish(runner, outcome)
}

pub async fn param(
    runner: &Runner<'_>,
    command: &SwarmParamCommands,
    non_interactive: bool,
    csv: bool,
) -> Result<()> {
    match command {
        SwarmParamCommands::Get(VariableName { names }) => {
            let names = match names {
                Some(names) => names.clone(),
                None => pick(runner, non_interactive, "<names>", async |cf| {
                    param::pick_names(cf, "Select parameters to show:").await
                })
                .await?,
            };
            param_get(runner, &names, csv).await
        }
        SwarmParamCommands::Set(VariableNameAndValue { params, store }) => {
            let params: HashMap<String, String> = match params {
                Some(params) => params.clone(),
                None => pick(runner, non_interactive, "<params>", async |cf| param::pick_values(cf).await).await?,
            };
            let results = runner
                .connected("Setting", async |cf| {
                    param::check_params_exist(cf, params.keys().map(String::as_str))?;
                    for (name, value) in &params {
                        param::set_value(cf, name, value).await?;
                        if *store {
                            cf.param.persistent_store(name).await?;
                        }
                    }
                    Ok(())
                })
                .await;
            finish_actions(runner, results, if *store { "set and stored" } else { "set" })
        }
        SwarmParamCommands::Store(VariableName { names }) => {
            let names = persistent_names(runner, names, non_interactive, "Select parameters to store:").await?;
            let results = runner
                .connected("Storing", async |cf| {
                    param::check_params_exist(cf, names.split(','))?;
                    for name in names.split(',') {
                        cf.param.persistent_store(name).await?;
                    }
                    Ok(())
                })
                .await;
            finish_actions(runner, results, "stored")
        }
        SwarmParamCommands::Clear(VariableName { names }) => {
            let names = persistent_names(runner, names, non_interactive, "Select parameters to clear:").await?;
            let results = runner
                .connected("Clearing", async |cf| {
                    param::check_params_exist(cf, names.split(','))?;
                    for name in names.split(',') {
                        cf.param.persistent_clear(name).await?;
                    }
                    Ok(())
                })
                .await;
            finish_actions(runner, results, "cleared")
        }
    }
}

async fn param_get(runner: &Runner<'_>, names: &str, csv: bool) -> Result<()> {
    if csv {
        let results = runner
            .connected("Reading", async |cf| {
                param::check_params_exist(cf, names.split(','))?;
                let mut rows = Vec::new();
                for name in names.split(',') {
                    rows.push(param::csv_fields(cf, name).await?);
                }
                Ok(rows)
            })
            .await;
        let (done, outcome) = split(results);
        println!("cf,uri,{}", param::PARAM_CSV_HEADER);
        for (i, rows) in &done {
            for fields in rows {
                let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
                csv_row_for(&runner.targets[*i], &fields);
            }
        }
        return finish(runner, outcome);
    }

    let results = runner
        .connected("Reading", async |cf| {
            param::check_params_exist(cf, names.split(','))?;
            param::get_rows(cf, names).await
        })
        .await;
    let (done, outcome) = split(results);
    let rows: Vec<_> = done
        .into_iter()
        .flat_map(|(i, rows)| rows.into_iter().map(move |r| (i, r)))
        .map(|(i, r)| row(runner, i, r))
        .collect();
    if !rows.is_empty() {
        // The Access column comes after CF and Name.
        param::print_get_table(&rows, 2);
    }
    finish(runner, outcome)
}

/// The persistent parameters given, or picked from the first Crazyflie.
async fn persistent_names(
    runner: &Runner<'_>,
    names: &Option<String>,
    non_interactive: bool,
    message: &str,
) -> Result<String> {
    match names {
        Some(names) => Ok(names.clone()),
        None => pick(runner, non_interactive, "<names>", async |cf| param::pick_persistent(cf, message).await).await,
    }
}

/// Let the user pick from what the first Crazyflie offers (its parameter
/// TOC). Connecting to it also puts its TOC in the cache for the others.
async fn pick<T>(
    runner: &Runner<'_>,
    non_interactive: bool,
    missing_arg: &str,
    picker: impl AsyncFn(&Crazyflie) -> Result<T>,
) -> Result<T> {
    crate::require_arg(non_interactive, missing_arg)?;
    let cf = runner.connect(&runner.targets[0]).await?;
    let picked = picker(&cf).await;
    cf.disconnect().await;
    picked
}

fn row<T: Tabled>(runner: &Runner<'_>, i: usize, row: T) -> SwarmRow<T> {
    SwarmRow { cf: runner.targets[i].name.clone(), row }
}

fn finish(runner: &Runner<'_>, outcome: Outcome) -> Result<()> {
    outcome.print_failures(&runner.targets);
    outcome.finish()
}

fn finish_actions(runner: &Runner<'_>, results: Vec<Result<()>>, done: &str) -> Result<()> {
    let (_, outcome) = split(results);
    outcome.print_actions(&runner.targets, done);
    outcome.finish()
}
