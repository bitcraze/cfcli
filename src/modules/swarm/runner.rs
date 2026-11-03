//! Running a command on many Crazyflies at once.
//!
//! The runner reaches the Crazyflies through the client API, like the
//! single-Crazyflie commands: with this computer's Crazyradios (direct mode,
//! the server runs inside cfcli) or through a connection server. The server
//! connects them, downloads each firmware's TOCs once and keeps the
//! Crazyflies of a channel on one Crazyradio.
//!
//! In direct mode the runner spreads the Crazyflies over the Crazyradios
//! itself (see [`radio::assign`]) and pins each to its radio, so that the
//! commands still done with crazyflie-lib or cfloader directly (bootloader,
//! memories) reach them on the same one. Through a server the server picks.
//!
//! A command runs on all the Crazyflies at once, also the ones that share a
//! Crazyradio. A few at a time is slower: the ones already connected keep
//! polling the radio while the next ones connect (25 on one radio: 3.0 s at 8
//! at a time, 1.4 s all at once, 2026-10-19). Only the bootloader steps run a
//! few per radio at a time ([`Runner::each_at_most`]). The results come back
//! in the order of the swarm file, whatever order the Crazyflies answer in.
//!
//! A Crazyflie that fails doesn't stop the others. [`Outcome::finish`] turns
//! the failures into the command's exit code.

use std::collections::BTreeMap;
use std::io::IsTerminal;

use anyhow::{Context, Result};
use crazyflie_connection_server::{Client, Crazyflie};
use crazyflie_lib::crazyflie_link::LinkContext;
use futures::stream::{self, StreamExt};
use tabled::Tabled;

use super::store::Swarm;
use crate::error::{self, CliError};
use crate::utils::display::csv_row;
use crate::utils::radio::{self, RadioUri};
use crate::Config;

/// One Crazyflie a command runs on.
pub struct Target {
    /// Its short name.
    pub name: String,
    /// Its URI as written in the swarm.
    pub uri: String,
    /// The URI to reach it with: the timeout from the settings, and in direct
    /// mode the Crazyradio it was given.
    pub link_uri: String,
    /// The Crazyradio it was given in direct mode. `None` for `usb://`, and
    /// through a server, which picks the radio.
    pub radio: Option<usize>,
    /// The targets of one lane share a Crazyradio: the radio in direct mode,
    /// the channel through a server. `None` for `usb://`.
    lane: Option<usize>,
}

impl Target {
    /// Whether it is reached by radio.
    pub fn is_radio(&self) -> bool {
        !self.uri.starts_with("usb://")
    }
}

/// A row of a normal listing with the Crazyflie's name in front.
#[derive(Tabled)]
pub struct SwarmRow<T: Tabled> {
    #[tabled(rename = "CF")]
    pub cf: String,
    #[tabled(inline)]
    pub row: T,
}

/// Print a CSV row with the Crazyflie's name and URI in front.
pub fn csv_row_for(target: &Target, fields: &[&str]) {
    let mut row = vec![target.name.as_str(), target.uri.as_str()];
    row.extend_from_slice(fields);
    csv_row(&row);
}

pub struct Runner<'a> {
    client: Client,
    link_context: &'a LinkContext,
    pub targets: Vec<Target>,
}

/// A Crazyflie the runner attached to.
pub struct Attached {
    pub cf: Crazyflie,
    /// Disconnect it when done (direct mode, where nobody else uses it), so
    /// that the next step can open its own link to it.
    disconnect: bool,
}

impl Attached {
    pub async fn disconnect(self) {
        let lib = self.cf.lib().filter(|_| self.disconnect);
        self.cf.detach();
        if let Some(lib) = lib {
            lib.disconnect().await;
        }
    }
}

impl<'a> Runner<'a> {
    /// The selected Crazyflies of a swarm, reached through `client`. In
    /// direct mode each is given a Crazyradio.
    pub async fn new(
        client: &Client,
        link_context: &'a LinkContext,
        config: &Config,
        swarm: &Swarm,
        selected: &[usize],
    ) -> Result<Runner<'a>> {
        let units: Vec<_> = selected.iter().map(|i| &swarm.units[*i]).collect();
        let parsed = units.iter().map(|unit| RadioUri::parse(&unit.uri)).collect::<Result<Vec<_>>>()?;

        let mut assigned = Vec::new().into_iter();
        let direct = client.is_in_process();
        if direct && parsed.iter().any(Option::is_some) {
            let links: Vec<radio::Link> = parsed
                .iter()
                .flatten()
                .map(|r| radio::Link { radio: r.radio, channel: r.channel })
                .collect();
            let available = radio::available_radios(link_context).await;
            if available.is_empty() {
                anyhow::bail!(CliError::Connection("no Crazyradio could be opened".to_string()));
            }
            let assignment = radio::assign(&links, &available);
            for warning in &assignment.warnings {
                eprintln!("Warning: {}", warning);
            }
            assigned = assignment.radios.into_iter();
        }

        let targets = units
            .iter()
            .zip(&parsed)
            .map(|(unit, parsed)| {
                let (uri, radio, lane) = match parsed {
                    Some(radio_uri) if direct => {
                        let radio = assigned.next().expect("one radio per radio URI");
                        (radio_uri.with_radio(radio), Some(radio), Some(radio))
                    }
                    Some(radio_uri) => (unit.uri.clone(), None, Some(usize::from(radio_uri.channel))),
                    None => (unit.uri.clone(), None, None),
                };
                Target {
                    name: unit.name.clone(),
                    uri: unit.uri.clone(),
                    link_uri: crate::with_timeout(config, uri),
                    radio,
                    lane,
                }
            })
            .collect();
        Ok(Runner { client: client.clone(), link_context, targets })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Whether this process drives the Crazyradios, so that crazyflie-lib
    /// and cfloader can be used directly.
    pub fn is_direct(&self) -> bool {
        self.client.is_in_process()
    }

    /// The link context, for the parts of `what` that use crazyflie-lib or
    /// cfloader directly. Direct mode only.
    pub fn direct(&self, what: &str) -> Result<&'a LinkContext> {
        if !self.is_direct() {
            anyhow::bail!(CliError::Connection(format!(
                "{} does not go through a connection server yet. Use --connection direct",
                what
            )));
        }
        Ok(self.link_context)
    }

    /// The link context, for code that only runs in direct mode (see
    /// [`Runner::direct`]).
    pub fn link_context(&self) -> &'a LinkContext {
        self.link_context
    }

    /// Run `op` for the targets with these indices. Each lane handles
    /// `per_radio` of them at a time, all lanes at once. The results come in
    /// the order they finish.
    pub(super) async fn run<R>(&self, indices: Vec<usize>, per_radio: usize, op: impl AsyncFn(usize) -> R) -> Vec<(usize, R)> {
        let mut by_lane: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for i in indices {
            by_lane.entry(self.targets[i].lane).or_default().push(i);
        }
        let op = &op;
        let runs = by_lane.into_values().map(|members| async move {
            stream::iter(members)
                .map(|i| async move { (i, op(i).await) })
                .buffer_unordered(per_radio)
                .collect::<Vec<_>>()
                .await
        });
        futures::future::join_all(runs).await.into_iter().flatten().collect()
    }

    /// Run `op` on every target at once and return the results in target
    /// order. `label` is shown next to the progress bar.
    pub async fn each<T>(&self, label: &str, op: impl AsyncFn(&Target) -> Result<T>) -> Vec<Result<T>> {
        self.each_at_most(label, usize::MAX, op).await
    }

    /// [`Runner::each`] with at most `per_radio` targets at a time on each
    /// Crazyradio, for an `op` that keeps a Crazyradio busier than a link
    /// to the firmware does.
    pub async fn each_at_most<T>(
        &self,
        label: &str,
        per_radio: usize,
        op: impl AsyncFn(&Target) -> Result<T>,
    ) -> Vec<Result<T>> {
        let all: Vec<usize> = (0..self.targets.len()).collect();
        self.each_of(&all, label, per_radio, op).await
    }

    /// [`Runner::each_at_most`] on the targets with these indices only.
    pub async fn each_of<T>(
        &self,
        indices: &[usize],
        label: &str,
        per_radio: usize,
        op: impl AsyncFn(&Target) -> Result<T>,
    ) -> Vec<Result<T>> {
        let bar = progress(label, indices.len());
        let mut results = self
            .run(indices.to_vec(), per_radio, async |i| {
                let result = op(&self.targets[i]).await;
                bar.inc(1);
                result
            })
            .await;
        bar.finish_and_clear();
        results.sort_by_key(|(i, _)| *i);
        results.into_iter().map(|(_, result)| result).collect()
    }

    /// Attach to a target. The server connects it unless another program
    /// already has.
    pub async fn attach(&self, target: &Target) -> Result<Attached> {
        let cf = self
            .client
            .attach(&target.link_uri)
            .await
            .context(CliError::Connection(format!("connecting to {}", target.link_uri)))?;
        Ok(Attached { cf, disconnect: self.client.is_direct() })
    }

    /// Attach to every target, run `op` on it and detach again.
    pub async fn attached<T>(&self, label: &str, op: impl AsyncFn(&Crazyflie, &Target) -> Result<T>) -> Vec<Result<T>> {
        self.each(label, async |target: &Target| {
            let attached = self.attach(target).await?;
            let result = op(&attached.cf, target).await;
            attached.disconnect().await;
            result
        })
        .await
    }

    /// Attach to every target and stay attached, for commands that talk to
    /// all the Crazyflies at the same time.
    pub async fn attach_all(&self, label: &str) -> Vec<Result<Attached>> {
        self.each(label, async |target: &Target| self.attach(target).await).await
    }

    /// The first target, e.g. to pick names from its TOC.
    pub async fn attach_first(&self) -> Result<Attached> {
        self.attach(&self.targets[0]).await
    }

    /// Which of these targets answer a packet over the radio, without
    /// connecting. Connected ones answer without radio traffic. Returns, in
    /// order, the Crazyradio each answered on (`Some(None)` over USB).
    pub async fn answering(&self, indices: &[usize]) -> Result<Vec<Option<Option<usize>>>> {
        let uris: Vec<&str> = indices.iter().map(|i| self.targets[*i].link_uri.as_str()).collect();
        let found = self.client.scan_selected(&uris).await.context(CliError::Connection("checking which Crazyflies answer".to_string()))?;
        Ok(indices
            .iter()
            .map(|i| {
                let key = crazyflie_connection_server::normalize_uri(&self.targets[*i].link_uri).ok()?.key;
                found.iter().find(|cf| cf.uri == key).map(|cf| cf.radio)
            })
            .collect())
    }

    /// Whether the target answers.
    pub async fn answers(&self, target: &Target) -> bool {
        let found = self.client.scan_selected(&[target.link_uri.as_str()]).await;
        found.is_ok_and(|found| !found.is_empty())
    }

    /// Fail with a connection error unless the target answers.
    pub async fn check_answers(&self, target: &Target) -> Result<()> {
        if !self.answers(target).await {
            anyhow::bail!(CliError::Connection(format!("{} doesn't answer", target.link_uri)));
        }
        Ok(())
    }

}

/// A progress bar on stderr, hidden when stderr isn't a terminal.
pub(super) fn progress(label: &str, total: usize) -> indicatif::ProgressBar {
    let bar = indicatif::ProgressBar::new(total as u64);
    bar.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{msg} [{bar:30.cyan/blue}] {pos}/{len}")
            .expect("valid template")
            .progress_chars("#>-"),
    );
    bar.set_message(label.to_string());
    if !std::io::stderr().is_terminal() {
        bar.set_draw_target(indicatif::ProgressDrawTarget::hidden());
    }
    bar
}

/// What went wrong on which Crazyflies.
pub struct Outcome {
    failures: Vec<(usize, anyhow::Error)>,
    total: usize,
}

/// Split results into the values of the Crazyflies that succeeded (with
/// their target index) and the outcome of the rest.
pub fn split<T>(results: Vec<Result<T>>) -> (Vec<(usize, T)>, Outcome) {
    let total = results.len();
    let mut done = Vec::new();
    let mut failures = Vec::new();
    for (i, result) in results.into_iter().enumerate() {
        match result {
            Ok(value) => done.push((i, value)),
            Err(e) => failures.push((i, e)),
        }
    }
    (done, Outcome { failures, total })
}

impl Outcome {
    /// Print each failed Crazyflie and why on stderr.
    pub fn print_failures(&self, targets: &[Target]) {
        for (i, error) in &self.failures {
            eprintln!("{}: {:#}", targets[*i].name, error);
        }
    }

    /// For commands that only do something: one line per Crazyflie in
    /// file order, `done` for those that succeeded and the error (on
    /// stderr) for the rest.
    pub fn print_actions(&self, targets: &[Target], done: &str) {
        for (i, target) in targets.iter().enumerate() {
            match self.failures.iter().find(|(f, _)| *f == i) {
                Some((_, error)) => eprintln!("{}: {:#}", target.name, error),
                None => println!("{}: {}", target.name, done),
            }
        }
    }

    /// The command's result:
    ///
    /// * every Crazyflie succeeded: success;
    /// * every Crazyflie failed in the same way (the same exit code): that
    ///   error, so a swarm that is switched off gives the usual connection
    ///   error, and a parameter no Crazyflie has the usual "not found";
    /// * otherwise [`CliError::SomeFailed`] (exit code 50).
    pub fn finish(self) -> Result<()> {
        if self.failures.is_empty() {
            return Ok(());
        }
        let codes: Vec<i32> = self.failures.iter().map(|(_, e)| error::classify_exit_code(e)).collect();
        let failed = self.failures.len();
        if failed == self.total && codes.iter().all(|code| *code == codes[0]) {
            let (_, first) = self.failures.into_iter().next().expect("there are failures");
            if self.total == 1 {
                return Err(first);
            }
            return Err(first.context(format!("all {} failed", super::crazyflies(self.total))));
        }
        Err(CliError::SomeFailed(format!("{} of {}", failed, super::crazyflies(self.total))).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection_error() -> anyhow::Error {
        // Built the way `Runner::connect` builds it.
        anyhow::Error::new(crazyflie_lib::Error::Disconnected)
            .context(CliError::Connection("connecting to radio://0/80/2M/E7E7E7E7E7".to_string()))
    }

    fn not_found() -> anyhow::Error {
        CliError::NotFound("parameter 'x.y'".to_string()).into()
    }

    fn code(result: Result<()>) -> i32 {
        result.map_or_else(|e| error::classify_exit_code(&e), |()| 0)
    }

    #[test]
    fn keeps_the_values_in_target_order() {
        let (done, outcome) = split(vec![Ok(1), Err(not_found()), Ok(3)]);
        assert_eq!(done, vec![(0, 1), (2, 3)]);
        assert_eq!(outcome.failures.len(), 1);
    }

    #[test]
    fn all_succeeded_is_success() {
        let (_, outcome) = split::<()>(vec![Ok(()), Ok(())]);
        assert_eq!(code(outcome.finish()), 0);
    }

    #[test]
    fn some_failed_is_exit_code_50() {
        let (_, outcome) = split(vec![Ok(()), Err(connection_error())]);
        assert_eq!(code(outcome.finish()), 50);
    }

    #[test]
    fn all_failed_the_same_way_keeps_that_exit_code() {
        let (_, outcome) = split::<()>(vec![Err(connection_error()), Err(connection_error())]);
        let error = outcome.finish().unwrap_err();
        assert_eq!(error::classify_exit_code(&error), 10);
        assert!(format!("{:#}", error).starts_with("all 2 Crazyflies failed: connection error"), "{:#}", error);

        let (_, outcome) = split::<()>(vec![Err(not_found())]);
        assert_eq!(code(outcome.finish()), 20);
    }

    #[test]
    fn all_failed_in_different_ways_is_exit_code_50() {
        let (_, outcome) = split::<()>(vec![Err(connection_error()), Err(not_found())]);
        assert_eq!(code(outcome.finish()), 50);
    }

    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "Name")]
        name: String,
        #[tabled(rename = "Value")]
        value: String,
    }

    #[test]
    fn swarm_rows_put_the_crazyflie_first() {
        let rows = vec![SwarmRow { cf: "CF-01".to_string(), row: Row { name: "a".to_string(), value: "1".to_string() } }];
        let table = crate::utils::display::table(&rows).to_string();
        let header = table.lines().next().unwrap();
        assert!(header.starts_with("CF "), "{}", table);
        assert!(header.contains("| Name") && header.contains("| Value"), "{}", table);
    }
}
