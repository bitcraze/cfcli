//! Lighthouse configuration module for cfcli
//!
//! This module provides functions to upload, download, compare and display
//! lighthouse base station geometry and calibration data.
//!
//! The file format is the one cflib (and so cfclient) reads and writes. Only
//! Lighthouse V2 base stations are supported.

use anyhow::{bail, Context, Result};
use crazyflie_lib::{
    subsystems::memory::{
        LighthouseBsCalibration, LighthouseBsGeometry, LighthouseCalibrationSweep,
        LighthouseMemory, MemoryType,
    },
    Crazyflie, Error,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::error::CliError;
use crate::utils::display::csv_row;

pub mod configs;

/// The `type` of a lighthouse configuration file.
pub const FILE_TYPE: &str = "lighthouse_system_configuration";
/// The file version written. cflib reads only this one.
pub const FILE_VERSION: &str = "1";
/// File versions read. Earlier cfcli versions wrote `'2'` for the same format.
const READ_VERSIONS: [&str; 2] = ["1", "2"];
/// `systemType` of Lighthouse V2 base stations, the only kind supported.
pub const SYSTEM_TYPE_V2: u8 = 2;

/// Base station IDs the lighthouse memory has room for. A firmware build
/// supports fewer (4 by default, see [`supported_base_stations`]).
const MAX_BASE_STATIONS: u8 = LighthouseMemory::MAX_BASE_STATIONS as u8;

fn make_progress(length: usize, label: &str, non_interactive: bool) -> indicatif::ProgressBar {
    use std::io::IsTerminal;
    let term_width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
    let bar_width = term_width.saturating_sub(50 + label.len());

    let pb = indicatif::ProgressBar::new(length as u64);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template(&format!(
                "{} [{{elapsed_precise}}] [{{bar:{}.cyan/blue}}] {{pos}}/{{len}} ({{eta}})",
                label, bar_width
            ))
            .unwrap()
            .progress_chars("#>-"),
    );
    if non_interactive || !std::io::stderr().is_terminal() {
        pb.set_draw_target(indicatif::ProgressDrawTarget::hidden());
    }
    pb
}

/// YAML file format for lighthouse configuration, compatible with cflib.
///
/// The maps are ordered by base station ID, so the same configuration always
/// gives the same file. Top-level fields cfcli doesn't know are kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LighthouseConfigFile {
    /// File type identifier, [`FILE_TYPE`]
    #[serde(rename = "type")]
    pub file_type: String,
    /// File format version
    #[serde(deserialize_with = "version_string")]
    pub version: String,
    /// System type, [`SYSTEM_TYPE_V2`] (cflib's default when it is missing)
    #[serde(rename = "systemType", default = "default_system_type")]
    pub system_type: u8,
    /// Geometry data for each base station
    #[serde(default)]
    pub geos: BTreeMap<u8, GeometryFileEntry>,
    /// Calibration data for each base station
    #[serde(default)]
    pub calibs: BTreeMap<u8, CalibrationFileEntry>,
    /// Fields cfcli doesn't use
    #[serde(flatten)]
    pub extra: serde_yaml::Mapping,
}

fn default_system_type() -> u8 {
    SYSTEM_TYPE_V2
}

/// cflib writes the version as a string; accept a plain number too.
fn version_string<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Version {
        Text(String),
        Number(u64),
    }
    Ok(match Version::deserialize(deserializer)? {
        Version::Text(text) => text,
        Version::Number(number) => number.to_string(),
    })
}

impl Default for LighthouseConfigFile {
    fn default() -> Self {
        Self {
            file_type: FILE_TYPE.to_string(),
            version: FILE_VERSION.to_string(),
            system_type: SYSTEM_TYPE_V2,
            geos: BTreeMap::new(),
            calibs: BTreeMap::new(),
            extra: serde_yaml::Mapping::new(),
        }
    }
}

impl LighthouseConfigFile {
    /// Parse and check a configuration file. The result is written back as
    /// the current file version.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let mut config: Self = serde_yaml::from_str(yaml).map_err(|e| {
            CliError::InvalidValue(format!("not a lighthouse configuration file: {}", e))
        })?;
        config.check()?;
        config.version = FILE_VERSION.to_string();
        Ok(config)
    }

    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("Failed to serialize lighthouse config to YAML")
    }

    /// Check what the Crazyflie needs: the file type and version, V2 base
    /// stations, base station IDs in range and finite numbers.
    pub fn check(&self) -> Result<()> {
        if self.file_type != FILE_TYPE {
            bail!(CliError::InvalidValue(format!(
                "not a lighthouse configuration file: type is '{}', not '{}'",
                self.file_type, FILE_TYPE
            )));
        }
        if !READ_VERSIONS.contains(&self.version.as_str()) {
            bail!(CliError::InvalidValue(format!(
                "unsupported lighthouse configuration file version '{}' (cfcli reads {})",
                self.version,
                READ_VERSIONS.join(" and ")
            )));
        }
        match self.system_type {
            SYSTEM_TYPE_V2 => {}
            1 => bail!(CliError::InvalidValue(
                "the file is for Lighthouse V1 base stations (systemType 1), only V2 is supported"
                    .to_string()
            )),
            other => bail!(CliError::InvalidValue(format!(
                "unknown lighthouse systemType {} (only 2, Lighthouse V2, is supported)",
                other
            ))),
        }
        if let Some(id) = self.ids().into_iter().find(|&id| id >= MAX_BASE_STATIONS) {
            bail!(CliError::InvalidValue(format!(
                "base station ID {} is out of range (0-{})",
                id,
                MAX_BASE_STATIONS - 1
            )));
        }
        for (id, geo) in &self.geos {
            let finite = geo.origin.iter().chain(geo.rotation.iter().flatten()).all(|v| v.is_finite());
            if !finite {
                bail!(CliError::InvalidValue(format!(
                    "the geometry of base station {} has a value that isn't a number",
                    id
                )));
            }
        }
        for (id, calib) in &self.calibs {
            if !calib.sweeps.iter().flat_map(SweepFileEntry::values).all(f32::is_finite) {
                bail!(CliError::InvalidValue(format!(
                    "the calibration of base station {} has a value that isn't a number",
                    id
                )));
            }
        }
        Ok(())
    }

    /// The base station IDs with geometry or calibration, in order.
    pub fn ids(&self) -> BTreeSet<u8> {
        self.geos.keys().chain(self.calibs.keys()).copied().collect()
    }
}

/// Geometry entry in the YAML file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryFileEntry {
    /// Origin position [x, y, z]
    pub origin: [f32; 3],
    /// Rotation matrix (3x3)
    pub rotation: [[f32; 3]; 3],
}

impl From<&LighthouseBsGeometry> for GeometryFileEntry {
    fn from(geo: &LighthouseBsGeometry) -> Self {
        Self {
            origin: geo.origin,
            rotation: geo.rotation_matrix,
        }
    }
}

impl From<&GeometryFileEntry> for LighthouseBsGeometry {
    fn from(entry: &GeometryFileEntry) -> Self {
        Self {
            origin: entry.origin,
            rotation_matrix: entry.rotation,
            valid: true,
        }
    }
}

/// Calibration sweep entry in the YAML file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepFileEntry {
    pub phase: f32,
    pub tilt: f32,
    pub curve: f32,
    pub gibmag: f32,
    pub gibphase: f32,
    pub ogeemag: f32,
    pub ogeephase: f32,
}

impl SweepFileEntry {
    fn values(&self) -> [f32; 7] {
        [
            self.phase,
            self.tilt,
            self.curve,
            self.gibmag,
            self.gibphase,
            self.ogeemag,
            self.ogeephase,
        ]
    }
}

impl From<&LighthouseCalibrationSweep> for SweepFileEntry {
    fn from(sweep: &LighthouseCalibrationSweep) -> Self {
        Self {
            phase: sweep.phase,
            tilt: sweep.tilt,
            curve: sweep.curve,
            gibmag: sweep.gibmag,
            gibphase: sweep.gibphase,
            ogeemag: sweep.ogeemag,
            ogeephase: sweep.ogeephase,
        }
    }
}

impl From<&SweepFileEntry> for LighthouseCalibrationSweep {
    fn from(entry: &SweepFileEntry) -> Self {
        Self {
            phase: entry.phase,
            tilt: entry.tilt,
            curve: entry.curve,
            gibmag: entry.gibmag,
            gibphase: entry.gibphase,
            ogeemag: entry.ogeemag,
            ogeephase: entry.ogeephase,
        }
    }
}

/// Calibration entry in the YAML file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationFileEntry {
    /// Base station UID
    pub uid: u32,
    /// Sweep calibration data
    pub sweeps: [SweepFileEntry; 2],
}

impl From<&LighthouseBsCalibration> for CalibrationFileEntry {
    fn from(calib: &LighthouseBsCalibration) -> Self {
        Self {
            uid: calib.uid,
            sweeps: [
                SweepFileEntry::from(&calib.sweeps[0]),
                SweepFileEntry::from(&calib.sweeps[1]),
            ],
        }
    }
}

impl From<&CalibrationFileEntry> for LighthouseBsCalibration {
    fn from(entry: &CalibrationFileEntry) -> Self {
        Self {
            sweeps: [
                LighthouseCalibrationSweep::from(&entry.sweeps[0]),
                LighthouseCalibrationSweep::from(&entry.sweeps[1]),
            ],
            uid: entry.uid,
            valid: true,
        }
    }
}

// ---- Comparing ----

/// How one part (geometry or calibration) of a base station compares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Part<D> {
    /// In neither.
    Absent,
    Same,
    Differs(D),
    /// In the file, not on the Crazyflie.
    OnlyInFile,
    /// On the Crazyflie, not in the file.
    OnlyOnCf,
}

impl<D> Part<D> {
    fn is_same(&self) -> bool {
        matches!(self, Part::Absent | Part::Same)
    }
}

/// How far a base station's geometry is from the file's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryDelta {
    pub moved_m: f64,
    pub turned_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalibrationDelta {
    /// Another base station's calibration: the Crazyflie takes the
    /// calibration from a base station whose UID differs from the one it has.
    Replaced { file_uid: u32, cf_uid: u32 },
    /// The same base station with other values.
    Values { uid: u32 },
}

/// One base station of a comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaseStationDiff {
    pub id: u8,
    pub geometry: Part<GeometryDelta>,
    pub calibration: Part<CalibrationDelta>,
}

impl BaseStationDiff {
    pub fn is_same(&self) -> bool {
        self.geometry.is_same() && self.calibration.is_same()
    }
}

/// Compare a configuration file with what a Crazyflie has, base station by
/// base station, for the IDs in either. Values are compared exactly, as the
/// f32 the Crazyflie stores.
pub fn compare(file: &LighthouseConfigFile, cf: &LighthouseConfigFile) -> Vec<BaseStationDiff> {
    let ids: BTreeSet<u8> = file.ids().union(&cf.ids()).copied().collect();
    ids.into_iter()
        .map(|id| BaseStationDiff {
            id,
            geometry: compare_part(file.geos.get(&id), cf.geos.get(&id), geometry_delta),
            calibration: compare_part(file.calibs.get(&id), cf.calibs.get(&id), calibration_delta),
        })
        .collect()
}

/// Whether a comparison found the Crazyflie up to date.
pub fn is_same(diffs: &[BaseStationDiff]) -> bool {
    diffs.iter().all(BaseStationDiff::is_same)
}

fn compare_part<T: PartialEq, D>(file: Option<&T>, cf: Option<&T>, delta: impl Fn(&T, &T) -> D) -> Part<D> {
    match (file, cf) {
        (None, None) => Part::Absent,
        (Some(_), None) => Part::OnlyInFile,
        (None, Some(_)) => Part::OnlyOnCf,
        (Some(f), Some(c)) if f == c => Part::Same,
        (Some(f), Some(c)) => Part::Differs(delta(f, c)),
    }
}

fn geometry_delta(file: &GeometryFileEntry, cf: &GeometryFileEntry) -> GeometryDelta {
    let moved_m = file
        .origin
        .iter()
        .zip(&cf.origin)
        .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    // The angle of the rotation between the two: trace(R1ᵀ R2) = 1 + 2 cos θ.
    let trace: f64 = file
        .rotation
        .iter()
        .flatten()
        .zip(cf.rotation.iter().flatten())
        .map(|(a, b)| *a as f64 * *b as f64)
        .sum();
    let turned_deg = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees();
    GeometryDelta { moved_m, turned_deg }
}

fn calibration_delta(file: &CalibrationFileEntry, cf: &CalibrationFileEntry) -> CalibrationDelta {
    if file.uid == cf.uid {
        CalibrationDelta::Values { uid: file.uid }
    } else {
        CalibrationDelta::Replaced {
            file_uid: file.uid,
            cf_uid: cf.uid,
        }
    }
}

pub(crate) fn describe_distance(m: f64) -> String {
    if m < 0.01 {
        format!("{:.1} mm", m * 1000.0)
    } else {
        format!("{:.1} cm", m * 100.0)
    }
}

fn describe_geometry(part: &Part<GeometryDelta>) -> String {
    match part {
        Part::Absent => "-".to_string(),
        Part::Same => "same".to_string(),
        Part::Differs(d) => format!(
            "moved {}, turned {:.2}°",
            describe_distance(d.moved_m),
            d.turned_deg
        ),
        Part::OnlyInFile => "not on the Crazyflie".to_string(),
        Part::OnlyOnCf => "not in the file".to_string(),
    }
}

fn describe_calibration(part: &Part<CalibrationDelta>) -> String {
    match part {
        Part::Absent => "-".to_string(),
        Part::Same => "same".to_string(),
        Part::Differs(CalibrationDelta::Replaced { file_uid, cf_uid }) => format!(
            "other base station: 0x{:08X} in the file, 0x{:08X} on the Crazyflie",
            file_uid, cf_uid
        ),
        Part::Differs(CalibrationDelta::Values { .. }) => "other values".to_string(),
        Part::OnlyInFile => "not on the Crazyflie".to_string(),
        Part::OnlyOnCf => "not in the file".to_string(),
    }
}

fn part_name<D>(part: &Part<D>) -> &'static str {
    match part {
        Part::Absent => "absent",
        Part::Same => "same",
        Part::Differs(_) => "differs",
        Part::OnlyInFile => "only_in_file",
        Part::OnlyOnCf => "only_on_cf",
    }
}

// ---- The Crazyflie ----

/// How many base stations the Crazyflie's firmware supports (IDs 0 to N-1),
/// from the size of its lighthouse memory, or None if that can't be told.
///
/// The firmware sizes the memory as the calibration start address plus one
/// packed calibration per base station it supports
/// (`CONFIG_DECK_LIGHTHOUSE_MAX_N_BS`, 4 unless built otherwise).
pub fn supported_base_stations(cf: &Crazyflie) -> Option<u8> {
    let memory = cf.memory.get_memories(Some(MemoryType::Lighthouse)).into_iter().next()?;
    supported_by_size(memory.size as usize)
}

fn supported_by_size(size: usize) -> Option<u8> {
    let calibrations = size.checked_sub(LighthouseMemory::CALIB_START_ADDR)?;
    if calibrations == 0 || calibrations % LighthouseBsCalibration::SIZE != 0 {
        return None;
    }
    let count = calibrations / LighthouseBsCalibration::SIZE;
    Some(count.min(MAX_BASE_STATIONS as usize) as u8)
}

async fn open_memory(cf: &Crazyflie) -> Result<LighthouseMemory> {
    let memories = cf.memory.get_memories(Some(MemoryType::Lighthouse));
    let Some(device) = memories.first() else {
        bail!(CliError::NotFound(
            "no lighthouse memory on the Crazyflie (is the Lighthouse deck attached?)".to_string()
        ));
    };
    match cf.memory.open_memory((*device).clone()).await {
        Some(Ok(m)) => Ok(m),
        Some(Err(e)) => bail!("Failed to open lighthouse memory: {}", e),
        None => bail!("Failed to open lighthouse memory"),
    }
}

/// Read the Crazyflie's configuration: the valid geometries and calibrations.
/// `progress` is called with (done, total) after each read.
pub async fn read_config(cf: &Crazyflie, mut progress: impl FnMut(usize, usize)) -> Result<LighthouseConfigFile> {
    let count = supported_base_stations(cf).unwrap_or(MAX_BASE_STATIONS);
    let memory = open_memory(cf).await?;
    let total = 2 * count as usize;
    let mut config = LighthouseConfigFile::default();
    let result: Result<()> = async {
        for id in 0..count {
            match memory.read_geometry(id).await {
                Ok(geo) if geo.valid => {
                    config.geos.insert(id, GeometryFileEntry::from(&geo));
                }
                Ok(_) => {}
                // Not supported by this firmware build (size unknown).
                Err(Error::MemoryError(_)) => {}
                Err(e) => return Err(e).with_context(|| format!("Failed to read geometry {}", id)),
            }
            progress(id as usize + 1, total);
        }
        for id in 0..count {
            match memory.read_calibration(id).await {
                Ok(calib) if calib.valid => {
                    config.calibs.insert(id, CalibrationFileEntry::from(&calib));
                }
                Ok(_) => {}
                Err(Error::MemoryError(_)) => {}
                Err(e) => return Err(e).with_context(|| format!("Failed to read calibration {}", id)),
            }
            progress(count as usize + id as usize + 1, total);
        }
        Ok(())
    }
    .await;
    cf.memory.close_memory(memory).await?;
    result?;
    Ok(config)
}

/// Refuse a configuration with base stations the Crazyflie's firmware
/// doesn't support, before anything is written.
pub fn check_supported(config: &LighthouseConfigFile, supported: Option<u8>) -> Result<()> {
    let Some(supported) = supported else {
        return Ok(());
    };
    let beyond: Vec<String> = config
        .ids()
        .into_iter()
        .filter(|&id| id >= supported)
        .map(|id| id.to_string())
        .collect();
    if !beyond.is_empty() {
        bail!(CliError::InvalidValue(format!(
            "the Crazyflie's firmware supports {} base stations (IDs 0-{}), the configuration also has {} {}; \
             nothing was written. Firmware built with a larger CONFIG_DECK_LIGHTHOUSE_MAX_N_BS supports more.",
            supported,
            supported.saturating_sub(1),
            if beyond.len() == 1 { "ID" } else { "IDs" },
            beyond.join(", ")
        )));
    }
    Ok(())
}

/// Write a configuration to the Crazyflie and persist it to its flash.
/// Base stations not in the configuration are written as invalid, so the
/// Crazyflie ends up with exactly this configuration. `progress` is called
/// with (done, total) after each write.
pub async fn write_config(
    cf: &Crazyflie,
    config: &LighthouseConfigFile,
    mut progress: impl FnMut(usize, usize),
) -> Result<()> {
    config.check()?;
    let supported = supported_base_stations(cf);
    check_supported(config, supported)?;
    let count = supported.unwrap_or(MAX_BASE_STATIONS);
    let total = 2 * count as usize;

    let memory = open_memory(cf).await?;
    let result: Result<()> = async {
        for id in 0..count {
            let geo = config.geos.get(&id).map(LighthouseBsGeometry::from).unwrap_or_default();
            match memory.write_geometry(id, &geo).await {
                Ok(()) => {}
                // Clearing an ID this firmware build doesn't have (size unknown).
                Err(Error::MemoryError(_)) if !config.geos.contains_key(&id) => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("Failed to write geometry for base station {}", id))
                }
            }
            progress(id as usize + 1, total);
        }
        for id in 0..count {
            let calib = config.calibs.get(&id).map(LighthouseBsCalibration::from).unwrap_or_default();
            match memory.write_calibration(id, &calib).await {
                Ok(()) => {}
                Err(Error::MemoryError(_)) if !config.calibs.contains_key(&id) => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to write calibration for base station {}", id))
                }
            }
            progress(count as usize + id as usize + 1, total);
        }
        Ok(())
    }
    .await;
    cf.memory.close_memory(memory).await?;
    result?;

    let ids: Vec<u8> = (0..count).collect();
    let persisted = cf
        .localization
        .lighthouse
        .persist_lighthouse_data(&ids, &ids)
        .await
        .context("Failed to persist lighthouse configuration to flash")?;
    if !persisted {
        bail!("Crazyflie reported failure while persisting lighthouse configuration to flash");
    }
    Ok(())
}

// ---- Commands ----

/// Read a configuration file.
pub fn load(path: &str) -> Result<LighthouseConfigFile> {
    let yaml = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read lighthouse config file: {}", path))?;
    LighthouseConfigFile::from_yaml(&yaml).with_context(|| format!("Failed to load {}", path))
}

/// The configuration piped in, or None when nothing is: stdin is a
/// terminal, or empty, as for a command run from a script or cron.
pub fn load_piped() -> Result<Option<LighthouseConfigFile>> {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    let mut yaml = String::new();
    std::io::stdin()
        .read_to_string(&mut yaml)
        .context("Failed to read lighthouse config from stdin")?;
    if yaml.trim().is_empty() {
        return Ok(None);
    }
    LighthouseConfigFile::from_yaml(&yaml)
        .context("Failed to load the configuration from stdin")
        .map(Some)
}

pub(crate) async fn read_with_progress(cf: &Crazyflie, non_interactive: bool) -> Result<LighthouseConfigFile> {
    let count = supported_base_stations(cf).unwrap_or(MAX_BASE_STATIONS);
    let progress_bar = make_progress(2 * count as usize, "Reading", non_interactive);
    let pb = progress_bar.clone();
    let config = read_config(cf, move |done, _| pb.set_position(done as u64)).await;
    progress_bar.finish_and_clear();
    config
}

/// Display lighthouse configuration from the Crazyflie
pub async fn display(cf: &Crazyflie, csv: bool, non_interactive: bool) -> Result<()> {
    let config = read_with_progress(cf, non_interactive).await?;
    print_config(&config, "Lighthouse Configuration", csv);
    Ok(())
}

/// Display lighthouse configuration from a YAML file (no connection needed)
pub fn display_file(file_path: &str, csv: bool) -> Result<()> {
    let config = load(file_path)?;
    print_config(&config, &format!("Lighthouse Configuration File: {}", file_path), csv);
    Ok(())
}

pub(crate) fn print_config(config: &LighthouseConfigFile, title: &str, csv: bool) {
    if csv {
        csv_row(&["section", "bs_id", "key", "value"]);
        for (id, geo) in &config.geos {
            emit_geometry_csv(*id, geo);
        }
        for (id, calib) in &config.calibs {
            emit_calibration_csv(*id, calib);
        }
        return;
    }

    println!("{}", title);
    println!("{}", "=".repeat(title.chars().count()));
    println!();

    if config.geos.is_empty() {
        println!("No geometry data.");
        println!();
    } else {
        println!("Geometry Data ({} base stations):", config.geos.len());
        println!("---------------------------------");
        for (id, geo) in &config.geos {
            println!("  Base Station {}:", id);
            println!("    Origin: [{:.4}, {:.4}, {:.4}]", geo.origin[0], geo.origin[1], geo.origin[2]);
            println!("    Rotation:");
            for row in &geo.rotation {
                println!("      [{:.6}, {:.6}, {:.6}]", row[0], row[1], row[2]);
            }
            println!();
        }
    }

    if config.calibs.is_empty() {
        println!("No calibration data.");
        println!();
    } else {
        println!("Calibration Data ({} base stations):", config.calibs.len());
        println!("------------------------------------");
        for (id, calib) in &config.calibs {
            println!("  Base Station {} (UID: 0x{:08X}):", id, calib.uid);
            for (i, sweep) in calib.sweeps.iter().enumerate() {
                println!("    Sweep {}:", i);
                println!("      phase={:.6}, tilt={:.6}, curve={:.6}", sweep.phase, sweep.tilt, sweep.curve);
                println!("      gibmag={:.6}, gibphase={:.6}", sweep.gibmag, sweep.gibphase);
                println!("      ogeemag={:.6}, ogeephase={:.6}", sweep.ogeemag, sweep.ogeephase);
            }
            println!();
        }
    }
}

fn emit_geometry_csv(bs_id: u8, geo: &GeometryFileEntry) {
    let bs = bs_id.to_string();
    let axes = ["x", "y", "z"];
    for (i, axis) in axes.iter().enumerate() {
        csv_row(&["geo", &bs, &format!("origin_{}", axis), &geo.origin[i].to_string()]);
    }
    for (r, row) in geo.rotation.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            csv_row(&["geo", &bs, &format!("rotation_{}_{}", r, c), &v.to_string()]);
        }
    }
}

fn emit_calibration_csv(bs_id: u8, calib: &CalibrationFileEntry) {
    let bs = bs_id.to_string();
    csv_row(&["cal", &bs, "uid", &calib.uid.to_string()]);
    // Only valid calibrations are read; the row is kept for existing readers.
    csv_row(&["cal", &bs, "valid", "true"]);
    let names = ["phase", "tilt", "curve", "gibmag", "gibphase", "ogeemag", "ogeephase"];
    for (i, sweep) in calib.sweeps.iter().enumerate() {
        for (name, value) in names.iter().zip(sweep.values()) {
            csv_row(&["cal", &bs, &format!("sweep{}_{}", i, name), &value.to_string()]);
        }
    }
}

/// Write a lighthouse configuration (see [`load`]) to the Crazyflie
pub async fn write(cf: &Crazyflie, config: &LighthouseConfigFile, non_interactive: bool) -> Result<()> {
    let count = supported_base_stations(cf).unwrap_or(MAX_BASE_STATIONS);
    let progress_bar = make_progress(2 * count as usize, "Writing", non_interactive);
    let pb = progress_bar.clone();
    let result = write_config(cf, config, move |done, _| pb.set_position(done as u64)).await;
    progress_bar.finish_and_clear();
    result?;
    println!(
        "Wrote geometry for {} and calibration for {} base station(s), cleared the rest, and stored it in flash",
        config.geos.len(),
        config.calibs.len()
    );
    Ok(())
}

/// Read lighthouse configuration from the Crazyflie as YAML (to file or stdout)
pub async fn read(cf: &Crazyflie, file_path: Option<&str>, non_interactive: bool) -> Result<()> {
    let config = read_with_progress(cf, non_interactive).await?;
    let yaml_content = config.to_yaml()?;
    let summary = format!(
        "Geometries: {}, calibrations: {}",
        config.geos.len(),
        config.calibs.len()
    );
    match file_path {
        Some(path) => {
            std::fs::write(path, yaml_content)
                .with_context(|| format!("Failed to write lighthouse config file: {}", path))?;
            println!("{}", summary);
        }
        None => {
            print!("{}", yaml_content);
            eprintln!("{}", summary);
        }
    }
    Ok(())
}

/// Compare the Crazyflie's configuration with a file (see [`load`]), named
/// `source` in messages. Fails with [`CliError::Differs`] unless they are
/// the same.
pub async fn check(
    cf: &Crazyflie,
    file: &LighthouseConfigFile,
    source: &str,
    csv: bool,
    non_interactive: bool,
) -> Result<()> {
    let on_cf = read_with_progress(cf, non_interactive).await?;
    let diffs = compare(file, &on_cf);

    if csv {
        csv_row(&["bs_id", "geometry", "moved_m", "turned_deg", "calibration", "file_uid", "cf_uid"]);
        for diff in &diffs {
            let (moved, turned) = match diff.geometry {
                Part::Differs(d) => (d.moved_m.to_string(), d.turned_deg.to_string()),
                _ => (String::new(), String::new()),
            };
            let uid = |calib: Option<&CalibrationFileEntry>| calib.map(|c| c.uid.to_string()).unwrap_or_default();
            csv_row(&[
                &diff.id.to_string(),
                part_name(&diff.geometry),
                &moved,
                &turned,
                part_name(&diff.calibration),
                &uid(file.calibs.get(&diff.id)),
                &uid(on_cf.calibs.get(&diff.id)),
            ]);
        }
    } else if diffs.is_empty() {
        println!("Neither the Crazyflie nor {} has any base stations.", source);
    } else {
        let rows: Vec<Vec<String>> = diffs
            .iter()
            .map(|d| {
                vec![
                    d.id.to_string(),
                    describe_geometry(&d.geometry),
                    describe_calibration(&d.calibration),
                ]
            })
            .collect();
        let header = ["BS".to_string(), "Geometry".to_string(), "Calibration".to_string()];
        crate::utils::display::print_table(&crate::utils::display::table_from_records(&header, &rows));
    }

    let differing = diffs.iter().filter(|d| !d.is_same()).count();
    if differing == 0 {
        if !csv {
            println!("The Crazyflie has the configuration in {}.", source);
        }
        return Ok(());
    }

    if !csv {
        if let Some(supported) = supported_base_stations(cf) {
            let beyond: Vec<String> =
                file.ids().into_iter().filter(|&id| id >= supported).map(|id| id.to_string()).collect();
            if !beyond.is_empty() {
                println!(
                    "The Crazyflie's firmware supports base stations 0-{}, so it can't have {}.",
                    supported.saturating_sub(1),
                    beyond.join(", ")
                );
            }
        }
        if diffs
            .iter()
            .any(|d| matches!(d.calibration, Part::Differs(CalibrationDelta::Replaced { .. })))
        {
            println!(
                "A base station whose UID differs has been replaced (the Crazyflie takes the calibration \
                 from the base station it sees); its geometry may need a new estimate."
            );
        }
    }
    bail!(CliError::Differs(format!(
        "the Crazyflie's lighthouse configuration differs from {} ({} of {} base stations)",
        source,
        differing,
        diffs.len()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
type: lighthouse_system_configuration
version: '2'
systemType: 2
geos:
  1:
    origin: [1.8885, -0.9296, 2.4064]
    rotation:
    - [-0.3296, -0.4936, -0.8048]
    - [0.4440, -0.8333, 0.3293]
    - [-0.8332, -0.2488, 0.4939]
  0:
    origin: [-0.5228, -0.8784, 2.2364123456789]
    rotation:
    - [1, 0, 0]
    - [0, 1, 0]
    - [0, 0, 1]
calibs:
  0:
    uid: 2360210604
    sweeps:
    - {phase: 0.0, tilt: -0.051, curve: 0.275, gibmag: -0.005, gibphase: 2.281, ogeemag: -0.184, ogeephase: 1.847}
    - {phase: -0.005, tilt: 0.051, curve: 0.211, gibmag: -0.004, gibphase: 2.219, ogeemag: 0.073, ogeephase: 2.213}
";

    fn parse(yaml: &str) -> Result<LighthouseConfigFile> {
        LighthouseConfigFile::from_yaml(yaml)
    }

    fn exit_code(result: Result<LighthouseConfigFile>) -> i32 {
        crate::error::classify_exit_code(&result.unwrap_err())
    }

    #[test]
    fn writes_version_1_and_reads_1_and_2() {
        let config = parse(FILE).unwrap();
        assert_eq!(config.version, "1");
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("version: '1'"), "{}", yaml);
        assert!(yaml.contains("type: lighthouse_system_configuration"), "{}", yaml);
        assert!(yaml.contains("systemType: 2"), "{}", yaml);
        assert!(parse(&FILE.replace("version: '2'", "version: '1'")).is_ok());
        assert!(parse(&FILE.replace("version: '2'", "version: 1")).is_ok());

        let unknown = parse(&FILE.replace("version: '2'", "version: '3'"));
        assert!(format!("{:#}", unknown.as_ref().unwrap_err()).contains("version '3'"));
        assert_eq!(exit_code(unknown), 30);
        assert!(parse(&FILE.replace("version: '2'\n", "")).is_err(), "version is required");
        assert!(parse(&FILE.replace("type: lighthouse_system_configuration\n", "")).is_err());
        assert!(parse(&FILE.replace("lighthouse_system_configuration", "swarm")).is_err());
    }

    #[test]
    fn only_lighthouse_v2() {
        assert_eq!(parse(&FILE.replace("systemType: 2\n", "")).unwrap().system_type, 2);
        let v1 = parse(&FILE.replace("systemType: 2", "systemType: 1"));
        assert!(format!("{:#}", v1.as_ref().unwrap_err()).contains("V1"));
        assert_eq!(exit_code(v1), 30);
        assert!(parse(&FILE.replace("systemType: 2", "systemType: 3")).is_err());
    }

    #[test]
    fn refuses_bad_ids_and_numbers() {
        assert!(parse(&FILE.replace("  1:\n    origin", "  16:\n    origin")).is_err());
        assert!(parse(&FILE.replace("1.8885", ".nan")).is_err());
        assert!(parse(&FILE.replace("tilt: -0.051", "tilt: .inf")).is_err());
    }

    #[test]
    fn serializing_is_stable() {
        let config = parse(FILE).unwrap();
        let yaml = config.to_yaml().unwrap();
        // Ordered by ID whatever the order in the file.
        assert!(yaml.find("\n  0:").unwrap() < yaml.find("\n  1:").unwrap(), "{}", yaml);
        let again = parse(&yaml).unwrap();
        assert_eq!(again, config);
        assert_eq!(again.to_yaml().unwrap(), yaml);
        // Values are what the Crazyflie stores: f32.
        assert_eq!(config.geos[&0].origin[2], 2.2364123456789_f64 as f32);
    }

    #[test]
    fn keeps_unknown_fields() {
        let config = parse(&format!("{}name: Lab\n", FILE)).unwrap();
        assert!(config.to_yaml().unwrap().contains("name: Lab"));
    }

    #[test]
    fn same_configuration() {
        let file = parse(FILE).unwrap();
        let diffs = compare(&file, &file.clone());
        assert_eq!(diffs.len(), 2);
        assert!(is_same(&diffs));
        assert_eq!(diffs[1].calibration, Part::Absent);
    }

    #[test]
    fn moved_and_turned() {
        let file = parse(FILE).unwrap();
        let mut cf = file.clone();
        let geo = cf.geos.get_mut(&0).unwrap();
        geo.origin[0] += 0.03;
        geo.origin[1] -= 0.04;
        // 90° about z.
        geo.rotation = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let diffs = compare(&file, &cf);
        let Part::Differs(delta) = diffs[0].geometry else {
            panic!("{:?}", diffs[0]);
        };
        assert!((delta.moved_m - 0.05).abs() < 1e-6, "{:?}", delta);
        assert!((delta.turned_deg - 90.0).abs() < 1e-4, "{:?}", delta);
        assert_eq!(diffs[0].calibration, Part::Same);
        assert!(diffs[1].is_same());
        assert!(!is_same(&diffs));
    }

    #[test]
    fn replaced_and_changed_calibration() {
        let file = parse(FILE).unwrap();
        let mut cf = file.clone();
        cf.calibs.get_mut(&0).unwrap().uid = 0x1234;
        assert_eq!(
            compare(&file, &cf)[0].calibration,
            Part::Differs(CalibrationDelta::Replaced {
                file_uid: 2360210604,
                cf_uid: 0x1234
            })
        );
        let mut cf = file.clone();
        cf.calibs.get_mut(&0).unwrap().sweeps[1].curve += 0.001;
        assert_eq!(
            compare(&file, &cf)[0].calibration,
            Part::Differs(CalibrationDelta::Values { uid: 2360210604 })
        );
    }

    #[test]
    fn missing_on_either_side() {
        let file = parse(FILE).unwrap();
        let mut cf = file.clone();
        cf.geos.remove(&1);
        cf.geos.insert(5, file.geos[&0].clone());
        cf.calibs.insert(5, file.calibs[&0].clone());
        let diffs = compare(&file, &cf);
        assert_eq!(diffs.iter().map(|d| d.id).collect::<Vec<_>>(), vec![0, 1, 5]);
        assert_eq!(diffs[1].geometry, Part::OnlyInFile);
        assert_eq!(diffs[2].geometry, Part::OnlyOnCf);
        assert_eq!(diffs[2].calibration, Part::OnlyOnCf);
    }

    #[test]
    fn supported_base_stations_from_the_memory_size() {
        // CONFIG_DECK_LIGHTHOUSE_MAX_N_BS 4 (the default) and 16.
        assert_eq!(supported_by_size(0x1000 + 4 * 61), Some(4));
        assert_eq!(supported_by_size(0x1000 + 16 * 61), Some(16));
        assert_eq!(supported_by_size(0x1000), None);
        assert_eq!(supported_by_size(0x800), None);
        assert_eq!(supported_by_size(0x1000 + 100), None);
    }

    #[test]
    fn refuses_more_base_stations_than_supported() {
        let mut config = parse(FILE).unwrap();
        assert!(check_supported(&config, Some(4)).is_ok());
        assert!(check_supported(&config, None).is_ok());
        config.geos.insert(4, config.geos[&0].clone());
        config.calibs.insert(6, config.calibs[&0].clone());
        let err = check_supported(&config, Some(4)).unwrap_err();
        assert!(format!("{:#}", err).contains("IDs 4, 6"), "{:#}", err);
        assert_eq!(crate::error::classify_exit_code(&err), 30);
    }
}
