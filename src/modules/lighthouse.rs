//! Lighthouse configuration module for cfcli
//!
//! This module provides functions to upload, download, and display
//! lighthouse base station geometry and calibration data.

use anyhow::{bail, Context, Result};
use crazyflie_lib::{
    subsystems::memory::{
        LighthouseBsCalibration, LighthouseBsGeometry, LighthouseCalibrationSweep,
        LighthouseMemory, MemoryType,
    },
    Crazyflie,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::utils::display::csv_row;

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

/// YAML file format for lighthouse configuration
/// Compatible with Python cflib format
#[derive(Debug, Serialize, Deserialize)]
pub struct LighthouseConfigFile {
    /// File type identifier
    #[serde(rename = "type", default = "default_file_type")]
    pub file_type: String,
    /// File format version
    #[serde(default = "default_version")]
    pub version: String,
    /// System type (1=V1, 2=V2)
    #[serde(rename = "systemType", default)]
    pub system_type: Option<u8>,
    /// Geometry data for each base station
    #[serde(default)]
    pub geos: HashMap<u8, GeometryFileEntry>,
    /// Calibration data for each base station
    #[serde(default)]
    pub calibs: HashMap<u8, CalibrationFileEntry>,
}

fn default_file_type() -> String {
    "lighthouse_system_configuration".to_string()
}

fn default_version() -> String {
    "2".to_string()
}

impl Default for LighthouseConfigFile {
    fn default() -> Self {
        Self {
            file_type: default_file_type(),
            version: default_version(),
            system_type: Some(2),
            geos: HashMap::new(),
            calibs: HashMap::new(),
        }
    }
}

/// Geometry entry in the YAML file
#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Serialize, Deserialize)]
pub struct SweepFileEntry {
    pub phase: f32,
    pub tilt: f32,
    pub curve: f32,
    pub gibmag: f32,
    pub gibphase: f32,
    pub ogeemag: f32,
    pub ogeephase: f32,
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
#[derive(Debug, Serialize, Deserialize)]
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

/// Display lighthouse configuration from the Crazyflie
pub async fn display(
    cf: &Crazyflie,
    csv: bool,
    non_interactive: bool,
) -> Result<()> {
    // Find lighthouse memory
    let memories = cf.memory.get_memories(Some(MemoryType::Lighthouse));
    let lighthouse_mem_device = match memories.first() {
        Some(m) => m,
        None => bail!("No lighthouse memory found on Crazyflie"),
    };

    if !csv {
        println!("Lighthouse Configuration");
        println!("========================");
        println!();
    } else {
        csv_row(&["section", "bs_id", "key", "value"]);
    }

    // Open lighthouse memory
    let lighthouse_mem: LighthouseMemory = match cf
        .memory
        .open_memory((*lighthouse_mem_device).clone())
        .await
    {
        Some(Ok(m)) => m,
        Some(Err(e)) => bail!("Failed to open lighthouse memory: {}", e),
        None => bail!("Failed to open lighthouse memory"),
    };

    // Read and display geometry data
    {
        if !csv {
            println!("Geometry Data:");
            println!("--------------");
        }

        let progress_bar = make_progress(LighthouseMemory::MAX_BASE_STATIONS, "Reading geometry", non_interactive);
        let pb = progress_bar.clone();
        let geometries = lighthouse_mem
            .read_all_geometries_with_progress(move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to read geometries")?;
        progress_bar.finish_and_clear();

        if geometries.is_empty() {
            if !csv {
                println!("  No valid geometry data found.");
            }
        } else {
            for bs_id in 0..LighthouseMemory::MAX_BASE_STATIONS as u8 {
                if let Some(geo) = geometries.get(&bs_id) {
                    if csv {
                        emit_geometry_csv(bs_id, geo);
                    } else {
                        println!("  Base Station {}:", bs_id);
                        println!("    Origin: [{:.4}, {:.4}, {:.4}]", geo.origin[0], geo.origin[1], geo.origin[2]);
                        println!("    Rotation:");
                        for row in &geo.rotation_matrix {
                            println!("      [{:.6}, {:.6}, {:.6}]", row[0], row[1], row[2]);
                        }
                        println!();
                    }
                }
            }
        }
    }

    // Read and display calibration data
    {
        if !csv {
            println!("Calibration Data:");
            println!("-----------------");
        }

        let progress_bar = make_progress(LighthouseMemory::MAX_BASE_STATIONS, "Reading calibration", non_interactive);
        let pb = progress_bar.clone();
        let calibrations = lighthouse_mem
            .read_all_calibrations_with_progress(move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to read calibrations")?;
        progress_bar.finish_and_clear();

        if calibrations.is_empty() {
            if !csv {
                println!("  No valid calibration data found.");
            }
        } else {
            for bs_id in 0..LighthouseMemory::MAX_BASE_STATIONS as u8 {
                if let Some(calib) = calibrations.get(&bs_id) {
                    if csv {
                        emit_calibration_csv(bs_id, calib);
                    } else {
                        println!("  Base Station {} (UID: 0x{:08X}):", bs_id, calib.uid);
                        for (i, sweep) in calib.sweeps.iter().enumerate() {
                            println!("    Sweep {}:", i);
                            println!("      phase={:.6}, tilt={:.6}, curve={:.6}",
                                sweep.phase, sweep.tilt, sweep.curve);
                            println!("      gibmag={:.6}, gibphase={:.6}",
                                sweep.gibmag, sweep.gibphase);
                            println!("      ogeemag={:.6}, ogeephase={:.6}",
                                sweep.ogeemag, sweep.ogeephase);
                        }
                        println!();
                    }
                }
            }
        }
    }

    // Close memory
    cf.memory.close_memory(lighthouse_mem).await?;

    Ok(())
}

fn emit_geometry_csv(bs_id: u8, geo: &LighthouseBsGeometry) {
    let bs = bs_id.to_string();
    let axes = ["x", "y", "z"];
    for (i, axis) in axes.iter().enumerate() {
        csv_row(&["geo", &bs, &format!("origin_{}", axis), &geo.origin[i].to_string()]);
    }
    for (r, row) in geo.rotation_matrix.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            csv_row(&["geo", &bs, &format!("rotation_{}_{}", r, c), &v.to_string()]);
        }
    }
}

fn emit_calibration_csv(bs_id: u8, calib: &LighthouseBsCalibration) {
    let bs = bs_id.to_string();
    csv_row(&["cal", &bs, "uid", &calib.uid.to_string()]);
    csv_row(&["cal", &bs, "valid", &calib.valid.to_string()]);
    for (i, sweep) in calib.sweeps.iter().enumerate() {
        let prefix = format!("sweep{}", i);
        csv_row(&["cal", &bs, &format!("{}_phase", prefix), &sweep.phase.to_string()]);
        csv_row(&["cal", &bs, &format!("{}_tilt", prefix), &sweep.tilt.to_string()]);
        csv_row(&["cal", &bs, &format!("{}_curve", prefix), &sweep.curve.to_string()]);
        csv_row(&["cal", &bs, &format!("{}_gibmag", prefix), &sweep.gibmag.to_string()]);
        csv_row(&["cal", &bs, &format!("{}_gibphase", prefix), &sweep.gibphase.to_string()]);
        csv_row(&["cal", &bs, &format!("{}_ogeemag", prefix), &sweep.ogeemag.to_string()]);
        csv_row(&["cal", &bs, &format!("{}_ogeephase", prefix), &sweep.ogeephase.to_string()]);
    }
}

/// Write lighthouse configuration from YAML (file or stdin) to the Crazyflie
pub async fn write(
    cf: &Crazyflie,
    file_path: Option<&str>,
    non_interactive: bool,
) -> Result<()> {
    // Read and parse YAML
    let yaml_content = match file_path {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read lighthouse config file: {}", path))?,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("Failed to read lighthouse config from stdin")?;
            buf
        }
    };

    let config: LighthouseConfigFile = serde_yaml::from_str(&yaml_content)
        .with_context(|| "Failed to parse lighthouse config YAML")?;

    // Find lighthouse memory
    let memories = cf.memory.get_memories(Some(MemoryType::Lighthouse));
    let lighthouse_mem_device = match memories.first() {
        Some(m) => m,
        None => bail!("No lighthouse memory found on Crazyflie"),
    };

    // Open lighthouse memory
    let lighthouse_mem: LighthouseMemory = match cf
        .memory
        .open_memory((*lighthouse_mem_device).clone())
        .await
    {
        Some(Ok(m)) => m,
        Some(Err(e)) => bail!("Failed to open lighthouse memory: {}", e),
        None => bail!("Failed to open lighthouse memory"),
    };

    // Upload geometry data
    if !config.geos.is_empty() {
        let geometries: HashMap<u8, LighthouseBsGeometry> = config
            .geos
            .iter()
            .map(|(&id, entry)| (id, LighthouseBsGeometry::from(entry)))
            .collect();

        let progress_bar = make_progress(geometries.len(), "Geometry", non_interactive);
        let pb = progress_bar.clone();
        lighthouse_mem
            .write_geometries_with_progress(&geometries, move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to write geometries")?;
        progress_bar.finish_with_message(format!("Uploaded {} geometries", geometries.len()));
    }

    // Upload calibration data
    if !config.calibs.is_empty() {
        let calibrations: HashMap<u8, LighthouseBsCalibration> = config
            .calibs
            .iter()
            .map(|(&id, entry)| (id, LighthouseBsCalibration::from(entry)))
            .collect();

        let progress_bar = make_progress(calibrations.len(), "Calibration", non_interactive);
        let pb = progress_bar.clone();
        lighthouse_mem
            .write_calibrations_with_progress(&calibrations, move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to write calibrations")?;
        progress_bar.finish_with_message(format!("Uploaded {} calibrations", calibrations.len()));
    }

    // Close memory
    cf.memory.close_memory(lighthouse_mem).await?;

    println!();
    println!("Lighthouse configuration uploaded successfully!");

    Ok(())
}

/// Read lighthouse configuration from the Crazyflie as YAML (to file or stdout)
pub async fn read(
    cf: &Crazyflie,
    file_path: Option<&str>,
    non_interactive: bool,
) -> Result<()> {
    // Find lighthouse memory
    let memories = cf.memory.get_memories(Some(MemoryType::Lighthouse));
    let lighthouse_mem_device = match memories.first() {
        Some(m) => m,
        None => bail!("No lighthouse memory found on Crazyflie"),
    };

    // Open lighthouse memory
    let lighthouse_mem: LighthouseMemory = match cf
        .memory
        .open_memory((*lighthouse_mem_device).clone())
        .await
    {
        Some(Ok(m)) => m,
        Some(Err(e)) => bail!("Failed to open lighthouse memory: {}", e),
        None => bail!("Failed to open lighthouse memory"),
    };

    let mut config = LighthouseConfigFile::default();

    // Read geometry data
    {
        let progress_bar = make_progress(LighthouseMemory::MAX_BASE_STATIONS, "Geometry", non_interactive);
        let pb = progress_bar.clone();
        let geometries = lighthouse_mem
            .read_all_geometries_with_progress(move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to read geometries")?;
        progress_bar.finish_with_message(format!("Read {} valid geometries", geometries.len()));

        config.geos = geometries
            .iter()
            .map(|(&id, geo)| (id, GeometryFileEntry::from(geo)))
            .collect();
    }

    // Read calibration data
    {
        let progress_bar = make_progress(LighthouseMemory::MAX_BASE_STATIONS, "Calibration", non_interactive);
        let pb = progress_bar.clone();
        let calibrations = lighthouse_mem
            .read_all_calibrations_with_progress(move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to read calibrations")?;
        progress_bar.finish_with_message(format!("Read {} valid calibrations", calibrations.len()));

        config.calibs = calibrations
            .iter()
            .map(|(&id, calib)| (id, CalibrationFileEntry::from(calib)))
            .collect();
    }

    // Close memory
    cf.memory.close_memory(lighthouse_mem).await?;

    let yaml_content = serde_yaml::to_string(&config)
        .context("Failed to serialize lighthouse config to YAML")?;

    match file_path {
        Some(path) => {
            std::fs::write(path, yaml_content)
                .with_context(|| format!("Failed to write lighthouse config file: {}", path))?;
            println!();
            println!("Found {} valid geometries and {} valid calibrations", config.geos.len(), config.calibs.len());
        }
        None => {
            print!("{}", yaml_content);
            eprintln!();
            eprintln!("Geometries: {}, calibrations: {}", config.geos.len(), config.calibs.len());
        }
    }

    Ok(())
}

/// Display lighthouse configuration from a YAML file (no connection needed)
pub fn display_file(file_path: &str) -> Result<()> {
    let yaml_content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read lighthouse config file: {}", file_path))?;

    let config: LighthouseConfigFile = serde_yaml::from_str(&yaml_content)
        .with_context(|| "Failed to parse lighthouse config YAML")?;

    println!("Lighthouse Configuration File: {}", file_path);
    println!("==============================");
    println!();

    if let Some(sys_type) = config.system_type {
        println!("System Type: {} ({})", sys_type, if sys_type == 1 { "V1" } else { "V2" });
    }
    println!();

    // Display geometry data
    if !config.geos.is_empty() {
        println!("Geometry Data ({} base stations):", config.geos.len());
        println!("---------------------------------");

        let mut sorted_ids: Vec<_> = config.geos.keys().collect();
        sorted_ids.sort();

        for bs_id in sorted_ids {
            if let Some(geo) = config.geos.get(bs_id) {
                println!("  Base Station {}:", bs_id);
                println!("    Origin: [{:.4}, {:.4}, {:.4}]", geo.origin[0], geo.origin[1], geo.origin[2]);
                println!("    Rotation:");
                for row in &geo.rotation {
                    println!("      [{:.6}, {:.6}, {:.6}]", row[0], row[1], row[2]);
                }
                println!();
            }
        }
    } else {
        println!("No geometry data in file.");
        println!();
    }

    // Display calibration data
    if !config.calibs.is_empty() {
        println!("Calibration Data ({} base stations):", config.calibs.len());
        println!("------------------------------------");

        let mut sorted_ids: Vec<_> = config.calibs.keys().collect();
        sorted_ids.sort();

        for bs_id in sorted_ids {
            if let Some(calib) = config.calibs.get(bs_id) {
                println!("  Base Station {} (UID: 0x{:08X}):", bs_id, calib.uid);
                for (i, sweep) in calib.sweeps.iter().enumerate() {
                    println!("    Sweep {}:", i);
                    println!("      phase={:.6}, tilt={:.6}, curve={:.6}",
                        sweep.phase, sweep.tilt, sweep.curve);
                    println!("      gibmag={:.6}, gibphase={:.6}",
                        sweep.gibmag, sweep.gibphase);
                    println!("      ogeemag={:.6}, ogeephase={:.6}",
                        sweep.ogeemag, sweep.ogeephase);
                }
                println!();
            }
        }
    } else {
        println!("No calibration data in file.");
        println!();
    }

    Ok(())
}
