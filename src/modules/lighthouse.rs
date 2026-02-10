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

use crate::utils::display::get_progressbar;

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
    geometry_only: bool,
    calibration_only: bool,
) -> Result<()> {
    // Find lighthouse memory
    let memories = cf.memory.get_memories(Some(MemoryType::Lighthouse));
    let lighthouse_mem_device = match memories.first() {
        Some(m) => m,
        None => bail!("No lighthouse memory found on Crazyflie"),
    };

    println!("Lighthouse Configuration");
    println!("========================");
    println!();

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
    if !calibration_only {
        println!("Geometry Data:");
        println!("--------------");

        let progress_bar = get_progressbar(LighthouseMemory::MAX_BASE_STATIONS, Some("Reading geometry"));
        let pb = progress_bar.clone();
        let geometries = lighthouse_mem
            .read_all_geometries_with_progress(move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to read geometries")?;
        progress_bar.finish_and_clear();

        if geometries.is_empty() {
            println!("  No valid geometry data found.");
        } else {
            for bs_id in 0..LighthouseMemory::MAX_BASE_STATIONS as u8 {
                if let Some(geo) = geometries.get(&bs_id) {
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

    // Read and display calibration data
    if !geometry_only {
        println!("Calibration Data:");
        println!("-----------------");

        let progress_bar = get_progressbar(LighthouseMemory::MAX_BASE_STATIONS, Some("Reading calibration"));
        let pb = progress_bar.clone();
        let calibrations = lighthouse_mem
            .read_all_calibrations_with_progress(move |completed, _total| {
                pb.set_position(completed as u64);
            })
            .await
            .context("Failed to read calibrations")?;
        progress_bar.finish_and_clear();

        if calibrations.is_empty() {
            println!("  No valid calibration data found.");
        } else {
            for bs_id in 0..LighthouseMemory::MAX_BASE_STATIONS as u8 {
                if let Some(calib) = calibrations.get(&bs_id) {
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

    // Close memory
    cf.memory.close_memory(lighthouse_mem).await?;

    Ok(())
}

/// Upload lighthouse configuration from a YAML file
pub async fn upload(
    cf: &Crazyflie,
    file_path: &str,
    geometry_only: bool,
    calibration_only: bool,
) -> Result<()> {
    // Read and parse YAML file
    let yaml_content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read lighthouse config file: {}", file_path))?;

    let config: LighthouseConfigFile = serde_yaml::from_str(&yaml_content)
        .with_context(|| "Failed to parse lighthouse config YAML")?;

    println!("Loaded lighthouse configuration from {}", file_path);
    if let Some(sys_type) = config.system_type {
        println!("  System type: {} ({})", sys_type, if sys_type == 1 { "V1" } else { "V2" });
    }
    println!("  Geometries: {}", config.geos.len());
    println!("  Calibrations: {}", config.calibs.len());
    println!();

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
    if !calibration_only && !config.geos.is_empty() {
        println!("Uploading geometry data...");
        let geometries: HashMap<u8, LighthouseBsGeometry> = config
            .geos
            .iter()
            .map(|(&id, entry)| (id, LighthouseBsGeometry::from(entry)))
            .collect();

        let progress_bar = get_progressbar(geometries.len(), Some("Uploading geometry"));
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
    if !geometry_only && !config.calibs.is_empty() {
        println!("Uploading calibration data...");
        let calibrations: HashMap<u8, LighthouseBsCalibration> = config
            .calibs
            .iter()
            .map(|(&id, entry)| (id, LighthouseBsCalibration::from(entry)))
            .collect();

        let progress_bar = get_progressbar(calibrations.len(), Some("Uploading calibration"));
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

/// Download lighthouse configuration to a YAML file
pub async fn download(
    cf: &Crazyflie,
    file_path: &str,
    geometry_only: bool,
    calibration_only: bool,
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
    if !calibration_only {
        println!("Reading geometry data...");
        let progress_bar = get_progressbar(LighthouseMemory::MAX_BASE_STATIONS, Some("Reading geometry"));
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
    if !geometry_only {
        println!("Reading calibration data...");
        let progress_bar = get_progressbar(LighthouseMemory::MAX_BASE_STATIONS, Some("Reading calibration"));
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

    // Write YAML file
    let yaml_content = serde_yaml::to_string(&config)
        .context("Failed to serialize lighthouse config to YAML")?;

    std::fs::write(file_path, yaml_content)
        .with_context(|| format!("Failed to write lighthouse config file: {}", file_path))?;

    println!();
    println!("Lighthouse configuration saved to {}", file_path);
    println!("  Geometries: {}", config.geos.len());
    println!("  Calibrations: {}", config.calibs.len());

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
