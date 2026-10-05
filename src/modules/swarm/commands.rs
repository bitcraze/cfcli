//! The normal cfcli commands, run on every Crazyflie of a swarm.
//!
//! Listings look like the normal ones with the Crazyflie's name added in
//! front (and its name and URI in front of each CSV row). Commands that only
//! do something print one line per Crazyflie. Crazyflies that fail are
//! reported on stderr, see [`Outcome::finish`] for the exit code.

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use anyhow::Result;
use crazyflie_lib::Crazyflie;
use futures::stream::StreamExt;
use tabled::Tabled;

use super::runner::{csv_row_for, split, Outcome, Runner, SwarmRow};
use crate::error::CliError;
use crate::modules::{bootloader, debug, deck, log, param, platform};
use crate::utils::display::{csv_row, print_table, table, table_from_records};
use crate::{
    AssertArgs, SwarmDebugCommands, SwarmDeckCommands, SwarmLogCommands, SwarmParamCommands,
    SwarmPlatformCommands, VariableName, VariableNameAndValue, VariablesAndPeriod,
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
    let results = runner.connected("Reading", async |cf, _| platform::info(cf).await).await;
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
    let results = runner.connected("Reading", async |cf, _| Ok(deck::decks(cf).await)).await;
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

pub async fn log(runner: &Runner<'_>, command: &SwarmLogCommands, non_interactive: bool, csv: bool) -> Result<()> {
    let SwarmLogCommands::Print(VariablesAndPeriod { names, period, once }) = command;
    let names = match names {
        Some(names) => names.clone(),
        None => pick(runner, non_interactive, "<names>", async |cf| log::pick_names(cf)).await?,
    };
    let names = log::split_names(&names);
    let period = u64::from(*period);
    if *once {
        log_once(runner, &names, period, csv).await
    } else {
        log_stream(runner, &names, period, csv).await
    }
}

/// One sample from each Crazyflie: a row per Crazyflie, a column per
/// variable.
async fn log_once(runner: &Runner<'_>, names: &[String], period: u64, csv: bool) -> Result<()> {
    let results = runner.connected("Reading", async |cf, _| log::sample(cf, names, period).await).await;
    let (done, outcome) = split(results);
    if csv {
        let mut header = vec!["cf", "uri", "timestamp_ms"];
        header.extend(names.iter().map(String::as_str));
        csv_row(&header);
        for (i, data) in &done {
            let timestamp = data.timestamp.to_string();
            let values = log::values(data, names);
            let mut fields = vec![timestamp.as_str()];
            fields.extend(values.iter().map(String::as_str));
            csv_row_for(&runner.targets[*i], &fields);
        }
    } else if !done.is_empty() {
        let mut header = vec!["CF".to_string()];
        header.extend(names.iter().cloned());
        let rows: Vec<Vec<String>> = done
            .iter()
            .map(|(i, data)| {
                let mut row = vec![runner.targets[*i].name.clone()];
                row.extend(log::values(data, names));
                row
            })
            .collect();
        print_table(&table_from_records(&header, &rows));
    }
    finish(runner, outcome)
}

/// Log from every Crazyflie at the same time, each line with the
/// Crazyflie's name in front, until stopped (Ctrl-C or `--timeout`) or until
/// every Crazyflie has dropped out.
async fn log_stream(runner: &Runner<'_>, names: &[String], period: u64, csv: bool) -> Result<()> {
    let connections = runner.connect_all("Connecting").await;
    let starts = futures::future::join_all(connections.into_iter().map(|connection| async move {
        let cf = connection?;
        match log::start(&cf, names, period).await {
            Ok(stream) => Ok((cf, stream)),
            Err(e) => {
                cf.disconnect().await;
                Err(e)
            }
        }
    }))
    .await;

    let mut results: Vec<Result<()>> = Vec::with_capacity(starts.len());
    let mut connections = Vec::new();
    let mut streams = Vec::new();
    for (i, start) in starts.into_iter().enumerate() {
        match start {
            Ok((cf, stream)) => {
                connections.push(cf);
                streams.push(Box::pin(futures::stream::unfold(Some(stream), move |stream| async move {
                    let stream = stream?;
                    match stream.next().await {
                        Ok(data) => Some(((i, Ok(data)), Some(stream))),
                        Err(e) => Some(((i, Err(e)), None)),
                    }
                })));
                results.push(Ok(()));
            }
            Err(e) => {
                eprintln!("{}: {:#}", runner.targets[i].name, e);
                results.push(Err(e));
            }
        }
    }

    if csv {
        let mut header = vec!["cf", "uri", "timestamp_ms"];
        header.extend(names.iter().map(String::as_str));
        csv_row(&header);
    }
    let width = runner.targets.iter().map(|t| t.name.len()).max().unwrap_or(0) + 1;
    let mut stdout = std::io::stdout();
    let mut merged = futures::stream::select_all(streams);
    while let Some((i, sample)) = merged.next().await {
        let target = &runner.targets[i];
        match sample {
            Ok(data) if csv => {
                let timestamp = data.timestamp.to_string();
                let values = log::values(&data, names);
                let mut fields = vec![timestamp.as_str()];
                fields.extend(values.iter().map(String::as_str));
                csv_row_for(target, &fields);
                // Flush per row, like `log print --csv`, so a consumer sees
                // the samples as they come.
                let _ = stdout.flush();
            }
            Ok(data) => println!("{:<width$} {:?}", format!("{}:", target.name), data, width = width),
            Err(e) => {
                eprintln!("{}: stopped: {}", target.name, e);
                results[i] = Err(CliError::Connection(format!("{} stopped logging: {}", target.link_uri, e)).into());
            }
        }
    }

    for cf in connections {
        cf.disconnect().await;
    }
    let (_, outcome) = split(results);
    outcome.finish()
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
    let results = runner.connected("Reading", async |cf, _| debug::assert_info(cf, wait).await).await;
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
                .connected("Setting", async |cf, _| {
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
                .connected("Storing", async |cf, _| {
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
                .connected("Clearing", async |cf, _| {
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
            .connected("Reading", async |cf, _| {
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
        .connected("Reading", async |cf, _| {
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
