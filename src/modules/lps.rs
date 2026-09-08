//! Loco Positioning System module for cfcli
//!
//! Displays live anchor data from the Loco Positioning v2 memory and
//! reads/writes anchor position configuration files.
//!
//! The YAML file format is the same one cfclient writes from the "Loco
//! Positioning" tab ("Configure positions" -> "Get from anchors" -> "Save to
//! file..."): a plain map of anchor id to `{x, y, z}` in meters.
//!
//! ```yaml
//! 0:
//!   x: 1.0
//!   y: 2.0
//!   z: 0.5
//! 1:
//!   x: 4.0
//!   y: 2.0
//!   z: 0.5
//! ```

use anyhow::{bail, Context, Result};
use crazyflie_lib::{
    subsystems::memory::{LocoMemory2, LocoSystemData, MemoryType},
    Crazyflie,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use crate::error::CliError;
use crate::utils::display::csv_row;

/// LPP short-packet type for setting an anchor position (firmware
/// `LPP_SHORT_ANCHORPOS`). Payload is this byte followed by 3x LE f32.
const LPP_TYPE_ANCHOR_POSITION: u8 = 0x01;

/// Tolerance (meters, per axis) for considering a position read back from an
/// anchor to match the one we wrote.
const ANCHOR_POS_TOLERANCE: f32 = 0.02;

/// Delay between resend rounds while waiting for anchors to confirm.
const RESEND_INTERVAL: Duration = Duration::from_millis(500);

/// One anchor entry in an anchor-positions YAML file (`{ id: {x, y, z} }`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AnchorPositionEntry {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl AnchorPositionEntry {
    fn position(&self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

impl From<[f32; 3]> for AnchorPositionEntry {
    fn from(p: [f32; 3]) -> Self {
        Self { x: p[0], y: p[1], z: p[2] }
    }
}

/// Anchor positions keyed by anchor id, ordered so the YAML output is stable.
pub type AnchorPositionFile = BTreeMap<u8, AnchorPositionEntry>;

fn positions_match(a: &[f32; 3], b: &[f32; 3]) -> bool {
    (a[0] - b[0]).abs() <= ANCHOR_POS_TOLERANCE
        && (a[1] - b[1]).abs() <= ANCHOR_POS_TOLERANCE
        && (a[2] - b[2]).abs() <= ANCHOR_POS_TOLERANCE
}

/// Progress reporting for the per-anchor read/write loops.
///
/// In a terminal this is a live `indicatif` bar. With `--non-interactive` (or
/// when stderr is redirected, where a bar would only render as garbage) it
/// falls back to plain lines on stderr so scripts and logs still see the
/// progress. Either way stdout is left alone for the actual data.
struct Progress {
    bar: Option<indicatif::ProgressBar>,
    label: &'static str,
    total: usize,
    reported: Option<usize>,
}

impl Progress {
    fn new(label: &'static str, total: usize, non_interactive: bool) -> Self {
        use std::io::IsTerminal;

        let bar = (!non_interactive && std::io::stderr().is_terminal()).then(|| {
            let term_width = terminal_size::terminal_size()
                .map(|(w, _)| w.0 as usize)
                .unwrap_or(80);
            let bar_width = term_width.saturating_sub(50 + label.len());

            let pb = indicatif::ProgressBar::new(total as u64);
            pb.set_style(
                indicatif::ProgressStyle::default_bar()
                    .template(&format!(
                        "{} [{{elapsed_precise}}] [{{bar:{}.cyan/blue}}] {{pos}}/{{len}} ({{eta}})",
                        label, bar_width
                    ))
                    .unwrap()
                    .progress_chars("#>-"),
            );
            // Draw the empty bar right away: the first step can take a while
            // and an empty bar beats a frozen terminal.
            pb.tick();
            pb
        });

        Self { bar, label, total, reported: None }
    }

    /// Report that `done` of `total` items are complete. Without a bar this
    /// prints a line, but only when the count actually moved.
    fn set(&mut self, done: usize) {
        match &self.bar {
            Some(bar) => bar.set_position(done as u64),
            None => {
                if self.reported != Some(done) {
                    eprintln!("{}: {}/{}", self.label, done, self.total);
                    self.reported = Some(done);
                }
            }
        }
    }

    /// Leave the completed bar on screen, the way the flash and memory
    /// commands do: the elapsed time and the final count are worth keeping
    /// once the operation is over. The caller prints the summary line after it.
    fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish();
        }
    }

    /// Leave the bar on screen at the position it actually reached, for an
    /// operation that gave up part-way. `finish` fills the bar to 100%, which
    /// would hide how many anchors were still missing.
    fn abandon(&self) {
        if let Some(bar) = &self.bar {
            bar.abandon();
        }
    }
}

/// Open the Loco Positioning v2 memory, failing with a useful message when
/// there is no LPS deck attached.
async fn open_loco_memory(cf: &Crazyflie) -> Result<LocoMemory2> {
    let memories = cf.memory.get_memories(Some(MemoryType::Loco2));

    if memories.is_empty() {
        bail!(CliError::NotFound(
            "Loco Positioning v2 memory (is the LPS deck attached?)".into()
        ));
    }

    match cf
        .memory
        .open_memory::<LocoMemory2>(memories[0].clone())
        .await
    {
        Some(Ok(m)) => Ok(m),
        Some(Err(e)) => bail!("Could not access Loco2 memory: {}", e),
        None => bail!("Loco2 memory not found"),
    }
}

/// Read the full anchor snapshot from the Crazyflie, closing the memory again.
async fn read_system_data(cf: &Crazyflie, non_interactive: bool) -> Result<LocoSystemData> {
    let loco_mem = open_loco_memory(cf).await?;
    let data = read_all_with_progress(&loco_mem, non_interactive).await;
    cf.memory.close_memory(loco_mem).await?;
    data
}

/// `LocoMemory2::read_all` unrolled so it can be reported as it goes: the
/// anchor pages are one memory transfer each, which is the slow part of every
/// read command once there are more than a handful of anchors.
async fn read_all_with_progress(
    loco_mem: &LocoMemory2,
    non_interactive: bool,
) -> Result<LocoSystemData> {
    let anchor_ids = loco_mem
        .read_id_list()
        .await
        .context("Failed to read the Loco anchor id list")?;
    let active_anchor_ids = loco_mem
        .read_active_id_list()
        .await
        .context("Failed to read the active Loco anchor id list")?;

    let mut progress = Progress::new("Reading anchors", anchor_ids.len(), non_interactive);
    let mut anchors = HashMap::new();
    for (done, &id) in anchor_ids.iter().enumerate() {
        let anchor = loco_mem
            .read_anchor_data(id)
            .await
            .with_context(|| format!("Failed to read data for anchor {}", id))?;
        anchors.insert(id, anchor);
        progress.set(done + 1);
    }
    progress.finish();

    Ok(LocoSystemData { anchor_ids, active_anchor_ids, anchors })
}

/// Display live anchor data (id, active/valid flags and position).
pub async fn display(cf: &Crazyflie, csv: bool, non_interactive: bool) -> Result<()> {
    let data = read_system_data(cf, non_interactive).await?;

    if csv {
        csv_row(&["id", "active", "valid", "x", "y", "z"]);
    } else {
        println!("Loco Positioning System - Anchor Data:");
        println!("  {:>3}  {:>6}  {:>5}  {}", "ID", "Active", "Valid", "Position (x, y, z)");
    }

    for &id in &data.anchor_ids {
        let is_active = data.active_anchor_ids.contains(&id);
        if let Some(anchor) = data.anchors.get(&id) {
            if csv {
                csv_row(&[
                    &id.to_string(),
                    &is_active.to_string(),
                    &anchor.is_valid.to_string(),
                    &anchor.position[0].to_string(),
                    &anchor.position[1].to_string(),
                    &anchor.position[2].to_string(),
                ]);
            } else {
                println!(
                    "  {:>3}  {:>6}  {:>5}  ({:.3}, {:.3}, {:.3})",
                    id,
                    if is_active { "yes" } else { "no" },
                    if anchor.is_valid { "yes" } else { "no" },
                    anchor.position[0],
                    anchor.position[1],
                    anchor.position[2],
                );
            }
        }
    }

    Ok(())
}

/// Read the anchor positions from the Crazyflie as YAML (to file or stdout).
///
/// This is the equivalent of cfclient's "Get from anchors" followed by "Save to
/// file...": the positions are the ones the Crazyflie has picked up from the
/// anchors over UWB.
pub async fn read(
    cf: &Crazyflie,
    file_path: Option<&str>,
    non_interactive: bool,
) -> Result<()> {
    let data = read_system_data(cf, non_interactive).await?;

    let mut positions = AnchorPositionFile::new();
    let mut skipped = Vec::new();

    for &id in &data.anchor_ids {
        if let Some(anchor) = data.anchors.get(&id) {
            if anchor.is_valid {
                positions.insert(id, AnchorPositionEntry::from(anchor.position));
            } else {
                skipped.push(id);
            }
        }
    }

    if positions.is_empty() {
        bail!(CliError::NotFound(
            "anchor positions on the Crazyflie. The anchors need to have been heard \
             from and have valid positions (see `cfcli loco display`)"
                .into()
        ));
    }

    let yaml_content = serde_yaml::to_string(&positions)
        .context("Failed to serialize anchor positions to YAML")?;

    match file_path {
        Some(path) => {
            std::fs::write(path, yaml_content)
                .with_context(|| format!("Failed to write anchor positions file: {}", path))?;
            println!("Wrote {} anchor positions to {}", positions.len(), path);
        }
        None => {
            print!("{}", yaml_content);
            eprintln!("Anchors: {}", positions.len());
        }
    }

    if !skipped.is_empty() {
        eprintln!(
            "Skipped {} anchor(s) without a valid position: {}",
            skipped.len(),
            id_list(skipped.iter())
        );
    }

    Ok(())
}

/// Parse an anchor-positions YAML file (or stdin when `file_path` is `None`).
fn parse_config(file_path: Option<&str>) -> Result<AnchorPositionFile> {
    let yaml_content = match file_path {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read anchor positions file: {}", path))?,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("Failed to read anchor positions from stdin")?;
            buf
        }
    };

    let positions: AnchorPositionFile = serde_yaml::from_str(&yaml_content)
        .context("Failed to parse anchor positions YAML")?;

    if positions.is_empty() {
        bail!(CliError::InvalidValue(
            "anchor positions file contains no anchors".into()
        ));
    }

    Ok(positions)
}

/// Write anchor positions from YAML (file or stdin) to the anchors.
///
/// The positions are pushed to each anchor with an LPP short packet relayed by
/// the Crazyflie. Those packets are best effort, so unless `verify` is false the
/// positions are read back from the Crazyflie and any anchor that has not picked
/// up its new position is resent until `timeout` expires.
pub async fn write(
    cf: &Crazyflie,
    file_path: Option<&str>,
    verify: bool,
    timeout: Duration,
    non_interactive: bool,
) -> Result<()> {
    let positions = parse_config(file_path)?;

    let targets: BTreeMap<u8, [f32; 3]> = positions
        .iter()
        .map(|(&id, entry)| (id, entry.position()))
        .collect();
    let total = targets.len();

    if !verify {
        let mut progress = Progress::new("Sending positions", total, non_interactive);
        send_positions(cf, &targets, Some(&mut progress)).await?;
        progress.finish();
        println!("Sent {} anchor positions (not verified)", total);
        return Ok(());
    }

    // Open the memory before sending anything: it both fails early when there
    // is no LPS deck (in which case the anchors would never hear us either) and
    // gives us the handle we need to read the positions back.
    let loco_mem = open_loco_memory(cf).await?;
    let result = send_and_verify(cf, &loco_mem, targets, timeout, non_interactive).await;
    cf.memory.close_memory(loco_mem).await?;
    result?;

    println!("All {} anchor positions written and confirmed", total);

    Ok(())
}

/// Send the positions and keep resending to the anchors that have not yet
/// reported the new position back, until they all confirm or `timeout` expires.
async fn send_and_verify(
    cf: &Crazyflie,
    loco_mem: &LocoMemory2,
    targets: BTreeMap<u8, [f32; 3]>,
    timeout: Duration,
    non_interactive: bool,
) -> Result<()> {
    let total = targets.len();

    // Push every anchor once up front, then keep resending to the stragglers.
    let mut sending = Progress::new("Sending positions", total, non_interactive);
    send_positions(cf, &targets, Some(&mut sending)).await?;
    sending.finish();

    let mut pending = targets;
    let deadline = Instant::now() + timeout;
    let mut confirmed = 0usize;
    let mut progress = Progress::new("Confirming anchors", total, non_interactive);

    loop {
        tokio::time::sleep(RESEND_INTERVAL).await;

        let data = loco_mem
            .read_all()
            .await
            .context("Failed to read anchor positions back from the Crazyflie")?;

        pending.retain(|id, target| {
            let matched = data
                .anchors
                .get(id)
                .is_some_and(|a| a.is_valid && positions_match(&a.position, target));
            if matched {
                confirmed += 1;
            }
            !matched
        });

        progress.set(confirmed);

        if pending.is_empty() {
            progress.finish();
            return Ok(());
        }

        if Instant::now() >= deadline {
            progress.abandon();
            bail!(CliError::Timeout(format!(
                "{} of {} anchor(s) did not confirm their new position within {}s: {}. \
                 Check that the anchors are powered and in range.",
                pending.len(),
                total,
                timeout.as_secs(),
                id_list(pending.keys())
            )));
        }

        // Resend to the stragglers only; the bar keeps showing confirmations.
        send_positions(cf, &pending, None).await?;
    }
}

/// Format anchor ids as a comma-separated list for messages.
fn id_list<'a>(ids: impl Iterator<Item = &'a u8>) -> String {
    ids.map(|id| id.to_string()).collect::<Vec<_>>().join(", ")
}

/// Send one LPP anchor-position packet per anchor.
async fn send_positions(
    cf: &Crazyflie,
    positions: &BTreeMap<u8, [f32; 3]>,
    mut progress: Option<&mut Progress>,
) -> Result<()> {
    for (sent, (&id, pos)) in positions.iter().enumerate() {
        let mut data = Vec::with_capacity(1 + 3 * 4);
        data.push(LPP_TYPE_ANCHOR_POSITION);
        for value in pos {
            data.extend_from_slice(&value.to_le_bytes());
        }
        cf.localization
            .loco_positioning
            .send_short_lpp_packet(id, &data)
            .await
            .with_context(|| format!("Failed to send position to anchor {}", id))?;
        if let Some(progress) = progress.as_mut() {
            progress.set(sent + 1);
        }
    }
    Ok(())
}

/// Display an anchor-positions YAML file (no connection needed).
pub fn display_file(file_path: &str) -> Result<()> {
    let positions = parse_config(Some(file_path))?;

    println!("Loco Anchor Positions File: {}", file_path);
    println!("===========================");
    println!();
    println!("  {:>3}  {}", "ID", "Position (x, y, z)");
    for (id, entry) in &positions {
        println!("  {:>3}  ({:.3}, {:.3}, {:.3})", id, entry.x, entry.y, entry.z);
    }
    println!();
    println!("{} anchors", positions.len());

    Ok(())
}
