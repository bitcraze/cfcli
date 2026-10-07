//! Running a command on many Crazyflies at once.
//!
//! The Crazyflies are spread over the Crazyradios (see
//! [`radio::assign`]), and each Crazyradio handles [`PER_RADIO`] of them at a
//! time. One at a time would take minutes for a big swarm, and all at once
//! leaves each link so little of the radio's time that connecting starts to
//! time out. The results come back in the order of the swarm file, whatever
//! order the Crazyflies answer in.
//!
//! A Crazyflie that fails doesn't stop the others. [`Outcome::finish`] turns
//! the failures into the command's exit code.
//!
//! Crazyflies running the same firmware have the same parameter and log
//! TOCs, so downloading them from more than one Crazyflie is wasted radio
//! time. [`Runner::connected`] connects using only the TOC cache at first,
//! lets one Crazyflie per TOC checksum download what is missing, and then
//! connects the rest from the cache.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crazyflie_lib::crazyflie_link::LinkContext;
use crazyflie_lib::{Crazyflie, TocCache};
use futures::stream::{self, StreamExt};
use tabled::Tabled;

use super::store::Swarm;
use crate::error::{self, CliError};
use crate::utils::display::csv_row;
use crate::utils::radio::{self, RadioUri};
use crate::{Config, ConfigTocCache};

/// Crazyflies handled at the same time on each Crazyradio.
const PER_RADIO: usize = 8;

/// Packets sent to a Crazyflie before it counts as not answering.
const ANSWER_ATTEMPTS: usize = 3;

/// One Crazyflie a command runs on.
pub struct Target {
    /// Its short name.
    pub name: String,
    /// Its URI as written in the swarm.
    pub uri: String,
    /// The URI to connect with: a real Crazyradio, and the timeout from the
    /// settings.
    pub link_uri: String,
    /// The Crazyradio it was given, `None` for `usb://`.
    pub radio: Option<usize>,
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
    link_context: &'a LinkContext,
    toc_cache: ConfigTocCache,
    pub targets: Vec<Target>,
}

impl<'a> Runner<'a> {
    /// The selected Crazyflies of a swarm, each given a Crazyradio.
    pub async fn new(
        link_context: &'a LinkContext,
        config: &Config,
        toc_cache: ConfigTocCache,
        swarm: &Swarm,
        selected: &[usize],
    ) -> Result<Runner<'a>> {
        let units: Vec<_> = selected.iter().map(|i| &swarm.units[*i]).collect();
        let parsed = units.iter().map(|unit| RadioUri::parse(&unit.uri)).collect::<Result<Vec<_>>>()?;

        let links: Vec<radio::Link> = parsed
            .iter()
            .flatten()
            .map(|r| radio::Link { radio: r.radio, channel: r.channel })
            .collect();
        let mut assigned = Vec::new().into_iter();
        if !links.is_empty() {
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
                let (radio, uri) = match parsed {
                    Some(radio_uri) => {
                        let radio = assigned.next().expect("one radio per radio URI");
                        (Some(radio), radio_uri.with_radio(radio))
                    }
                    None => (None, unit.uri.clone()),
                };
                Target {
                    name: unit.name.clone(),
                    uri: unit.uri.clone(),
                    link_uri: crate::with_timeout(config, uri),
                    radio,
                }
            })
            .collect();
        Ok(Runner { link_context, toc_cache, targets })
    }

    pub fn link_context(&self) -> &'a LinkContext {
        self.link_context
    }

    /// Connect to a Crazyflie the same way the other cfcli commands do,
    /// downloading the TOCs that aren't in the cache.
    pub async fn connect(&self, target: &Target) -> Result<Crazyflie> {
        self.connect_with(target, self.toc_cache.clone()).await
    }

    async fn connect_with(&self, target: &Target, toc_cache: impl TocCache) -> Result<Crazyflie> {
        Crazyflie::connect_from_uri(self.link_context, &target.link_uri, toc_cache)
            .await
            .context(CliError::Connection(format!("connecting to {}", target.link_uri)))
    }

    /// Run `op` for the targets with these indices. Each Crazyradio handles
    /// [`PER_RADIO`] of them at a time, all Crazyradios at once. The results
    /// come in the order they finish.
    async fn run<R>(&self, indices: Vec<usize>, op: impl AsyncFn(usize) -> R) -> Vec<(usize, R)> {
        let mut by_radio: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for i in indices {
            by_radio.entry(self.targets[i].radio).or_default().push(i);
        }
        let op = &op;
        let runs = by_radio.into_values().map(|members| async move {
            stream::iter(members)
                .map(|i| async move { (i, op(i).await) })
                .buffer_unordered(PER_RADIO)
                .collect::<Vec<_>>()
                .await
        });
        futures::future::join_all(runs).await.into_iter().flatten().collect()
    }

    /// Run `op` on every target and return the results in target order.
    /// `label` is shown next to the progress bar.
    pub async fn each<T>(&self, label: &str, op: impl AsyncFn(&Target) -> Result<T>) -> Vec<Result<T>> {
        let bar = progress(label, self.targets.len());
        let mut results = self
            .run((0..self.targets.len()).collect(), async |i| {
                let result = op(&self.targets[i]).await;
                bar.inc(1);
                result
            })
            .await;
        bar.finish_and_clear();
        results.sort_by_key(|(i, _)| *i);
        results.into_iter().map(|(_, result)| result).collect()
    }

    /// Connect to every target, run `op` on it and disconnect again. Each
    /// TOC missing from the cache is downloaded from one Crazyflie only.
    pub async fn connected<T>(
        &self,
        label: &str,
        op: impl AsyncFn(&Crazyflie, &Target) -> Result<T>,
    ) -> Vec<Result<T>> {
        self.connect_each(label, async |cf: Crazyflie, target: &Target| {
            let result = op(&cf, target).await;
            cf.disconnect().await;
            result
        })
        .await
        .into_iter()
        .map(|result| result.and_then(|inner| inner))
        .collect()
    }

    /// Connect to every target and keep the connections, for commands that
    /// talk to all the Crazyflies at the same time. Each TOC missing from
    /// the cache is downloaded from one Crazyflie only.
    pub async fn connect_all(&self, label: &str) -> Vec<Result<Crazyflie>> {
        self.connect_each(label, async |cf: Crazyflie, _: &Target| cf).await
    }

    /// Connect to every target and hand the connection to `then`, which
    /// owns it from there. Each TOC missing from the cache is downloaded
    /// from one Crazyflie only.
    async fn connect_each<R>(&self, label: &str, then: impl AsyncFn(Crazyflie, &Target) -> R) -> Vec<Result<R>> {
        let connect_then = async |target: &Target| -> Result<R> {
            let cf = self.connect(target).await?;
            Ok(then(cf, target).await)
        };
        // Without the cache every Crazyflie downloads its TOCs anyway.
        if self.toc_cache.no_toc_cache {
            return self.each(label, connect_then).await;
        }

        let total = self.targets.len();
        let bar = progress(label, total);
        let mut results: Vec<Option<Result<R>>> = std::iter::repeat_with(|| None).take(total).collect();
        let mut pending: Vec<usize> = (0..total).collect();
        while !pending.is_empty() {
            // Every pending Crazyflie whose TOCs are all in the cache.
            let probes = self
                .run(pending, async |i| {
                    let probe = ProbeCache::new(self.toc_cache.clone());
                    let cf = match self.connect_with(&self.targets[i], probe.clone()).await {
                        Ok(cf) => cf,
                        Err(e) => return Probe::Done(Err(e)),
                    };
                    let missed = probe.missed();
                    if missed.is_empty() {
                        Probe::Done(Ok(then(cf, &self.targets[i]).await))
                    } else {
                        cf.disconnect().await;
                        Probe::Missed(missed)
                    }
                })
                .await;

            // One Crazyflie per missing TOC downloads it, the others with
            // the same TOC try again once it is in the cache. Every round
            // finishes at least one Crazyflie per missing TOC, so this ends.
            let mut seen: Vec<Vec<Vec<u8>>> = Vec::new();
            let mut downloaders = Vec::new();
            pending = Vec::new();
            for (i, probe) in probes {
                match probe {
                    Probe::Done(result) => {
                        results[i] = Some(result);
                        bar.inc(1);
                    }
                    Probe::Missed(keys) if seen.contains(&keys) => pending.push(i),
                    Probe::Missed(keys) => {
                        seen.push(keys);
                        downloaders.push(i);
                    }
                }
            }
            for (i, result) in self.run(downloaders, async |i| connect_then(&self.targets[i]).await).await {
                results[i] = Some(result);
                bar.inc(1);
            }
            pending.sort_unstable();
        }

        bar.finish_and_clear();
        results.into_iter().map(|result| result.expect("every target ran")).collect()
    }
}

impl Target {
    /// Whether the Crazyflie answers a packet over the radio, without
    /// connecting to it. `usb://` targets aren't checked and count as
    /// answering: opening a link to a missing USB device fails anyway.
    pub async fn answers(&self, link_context: &LinkContext) -> bool {
        if self.radio.is_none() {
            return true;
        }
        for _ in 0..ANSWER_ATTEMPTS {
            if matches!(link_context.scan_selected(vec![self.link_uri.as_str()]).await, Ok(found) if !found.is_empty()) {
                return true;
            }
        }
        false
    }

    /// Fail with a connection error unless the Crazyflie answers.
    pub async fn check_answers(&self, link_context: &LinkContext) -> Result<()> {
        if !self.answers(link_context).await {
            anyhow::bail!(CliError::Connection(format!("{} doesn't answer", self.link_uri)));
        }
        Ok(())
    }
}

/// How connecting with only the TOC cache went.
enum Probe<T> {
    /// Connected (or failed to), and the connection was handed over.
    Done(Result<T>),
    /// These TOCs (cache keys) weren't in the cache.
    Missed(Vec<Vec<u8>>),
}

/// A TOC cache that never makes the Crazyflie download a TOC. A miss is
/// noted and answered with an empty TOC, so the connection finishes right
/// away (an empty parameter TOC also skips reading all the values); the
/// caller then disconnects and tries again once the TOC is in the cache.
#[derive(Clone)]
struct ProbeCache {
    inner: ConfigTocCache,
    missed: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl ProbeCache {
    fn new(inner: ConfigTocCache) -> Self {
        ProbeCache { inner, missed: Arc::default() }
    }

    /// The keys that weren't in the cache, sorted so that two Crazyflies
    /// missing the same TOCs compare equal.
    fn missed(&self) -> Vec<Vec<u8>> {
        let mut missed = self.missed.lock().unwrap().clone();
        missed.sort();
        missed
    }
}

impl TocCache for ProbeCache {
    fn get_toc(&self, key: &[u8]) -> Option<String> {
        self.inner.get_toc(key).or_else(|| {
            self.missed.lock().unwrap().push(key.to_vec());
            Some("{}".to_string())
        })
    }

    fn store_toc(&self, key: &[u8], toc: &str) {
        self.inner.store_toc(key, toc);
    }
}

/// A progress bar on stderr, hidden when stderr isn't a terminal.
fn progress(label: &str, total: usize) -> indicatif::ProgressBar {
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

    #[test]
    fn probe_cache_answers_a_miss_with_an_empty_toc() {
        let config = Config {
            uri: String::new(),
            toc_cache: [("01aabbccdd".to_string(), "{\"a\":[1,8]}".to_string())].into(),
            timeout_ms: None,
            addresses: Vec::new(),
            swarm: None,
            sync: None,
        };
        let probe = ProbeCache::new(ConfigTocCache::new(config, false));
        assert_eq!(probe.get_toc(&[1, 0xaa, 0xbb, 0xcc, 0xdd]).as_deref(), Some("{\"a\":[1,8]}"));
        assert!(probe.missed().is_empty());
        assert_eq!(probe.get_toc(&[1, 2, 3, 4, 5]).as_deref(), Some("{}"));
        assert_eq!(probe.get_toc(&[1, 0, 0, 0, 0]).as_deref(), Some("{}"));
        assert_eq!(probe.missed(), vec![vec![1, 0, 0, 0, 0], vec![1, 2, 3, 4, 5]]);
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
