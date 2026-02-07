//! Trajectory upload module for cfcli

use anyhow::{bail, Context, Result};
use crazyflie_lib::{
    subsystems::memory::{MemoryType, Poly, Poly4D, TrajectoryMemory},
    Crazyflie,
};
use serde::Deserialize;

use crate::utils::display::get_progressbar;

/// YAML format for a trajectory file
#[derive(Debug, Deserialize)]
pub struct TrajectoryFile {
    /// List of trajectory segments
    pub segments: Vec<TrajectorySegment>,
}

/// A single trajectory segment in the YAML file
///
/// Format matches the Python uav_trajectories tool output:
/// duration, x^0..x^7, y^0..y^7, z^0..z^7, yaw^0..yaw^7
#[derive(Debug, Deserialize)]
pub struct TrajectorySegment {
    /// Duration of this segment in seconds
    pub duration: f32,
    /// X polynomial coefficients (8 values)
    pub x: [f32; 8],
    /// Y polynomial coefficients (8 values)
    pub y: [f32; 8],
    /// Z polynomial coefficients (8 values)
    pub z: [f32; 8],
    /// Yaw polynomial coefficients (8 values)
    pub yaw: [f32; 8],
}

impl From<&TrajectorySegment> for Poly4D {
    fn from(seg: &TrajectorySegment) -> Self {
        Poly4D::new(
            seg.duration,
            Poly::new(seg.x),
            Poly::new(seg.y),
            Poly::new(seg.z),
            Poly::new(seg.yaw),
        )
    }
}

pub async fn upload(cf: &Crazyflie, file_path: &str, trajectory_id: u8, offset: u32) -> Result<()> {
    // Read and parse the YAML file
    let yaml_content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read trajectory file: {}", file_path))?;

    let trajectory_file: TrajectoryFile = serde_yaml::from_str(&yaml_content)
        .with_context(|| "Failed to parse trajectory YAML")?;

    println!(
        "Loaded trajectory with {} segments from {}",
        trajectory_file.segments.len(),
        file_path
    );

    // Calculate total duration
    let total_duration: f32 = trajectory_file.segments.iter().map(|s| s.duration).sum();
    println!("Total trajectory duration: {:.2}s", total_duration);

    // Find the trajectory memory
    let memories = cf.memory.get_memories(Some(MemoryType::Trajectory));
    let trajectory_mem_device = match memories.first() {
        Some(m) => m,
        None => bail!("No trajectory memory found on Crazyflie"),
    };

    println!("Found trajectory memory (ID: {})", trajectory_mem_device.memory_id);

    // Open the trajectory memory
    let trajectory_mem: TrajectoryMemory = match cf
        .memory
        .open_memory((*trajectory_mem_device).clone())
        .await
    {
        Some(Ok(m)) => m,
        Some(Err(e)) => bail!("Failed to initialize trajectory memory: {}", e),
        None => bail!("Failed to open trajectory memory"),
    };

    // Convert segments to Poly4D
    let poly4d_segments: Vec<Poly4D> = trajectory_file
        .segments
        .iter()
        .map(Poly4D::from)
        .collect();

    // Calculate total bytes to write (each Poly4D is 132 bytes)
    let total_bytes = poly4d_segments.len() * 132;

    // Upload the trajectory with progress
    println!(
        "Uploading trajectory to offset 0x{:04X}...",
        offset
    );

    let progress_bar = get_progressbar(total_bytes, Some("Uploading"));
    let pb = progress_bar.clone();
    let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
        pb.set_position(bytes_written as u64);
    };

    let bytes_written = trajectory_mem
        .write_uncompressed_with_progress(&poly4d_segments, offset as usize, progress_callback)
        .await
        .with_context(|| "Failed to write trajectory to memory")?;

    progress_bar.finish_with_message(format!("Uploaded {} bytes", bytes_written));

    // Define the trajectory with the high-level commander
    println!(
        "Defining trajectory ID {} with {} pieces at offset {}...",
        trajectory_id,
        poly4d_segments.len(),
        offset
    );

    cf.high_level_commander
        .define_trajectory(
            trajectory_id,
            offset,
            poly4d_segments.len() as u8,
            None,
        )
        .await
        .with_context(|| "Failed to define trajectory")?;

    // Close the memory
    cf.memory.close_memory(trajectory_mem).await?;

    println!("Trajectory uploaded and defined successfully!");
    println!(
        "Use trajectory ID {} to start the trajectory (duration: {:.2}s)",
        trajectory_id, total_duration
    );

    Ok(())
}

pub async fn run(
    cf: &Crazyflie,
    trajectory_id: u8,
    time_scale: f32,
    relative_position: bool,
    relative_yaw: bool,
    reversed: bool,
) -> Result<()> {
    println!("Starting trajectory {}...", trajectory_id);
    println!("  Time scale: {}", time_scale);
    println!("  Relative position: {}", relative_position);
    println!("  Relative yaw: {}", relative_yaw);
    println!("  Reversed: {}", reversed);

    cf.high_level_commander
        .start_trajectory(
            trajectory_id,
            time_scale,
            relative_position,
            relative_yaw,
            reversed,
            None,
        )
        .await
        .with_context(|| "Failed to start trajectory")?;

    println!("Trajectory {} started!", trajectory_id);

    Ok(())
}

/// Display the contents of a trajectory YAML file
pub fn display_file(file_path: &str) -> Result<()> {
    let yaml_content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read trajectory file: {}", file_path))?;

    let trajectory_file: TrajectoryFile = serde_yaml::from_str(&yaml_content)
        .with_context(|| "Failed to parse trajectory YAML")?;

    println!("Trajectory file: {}", file_path);
    println!("================");
    println!();

    let total_duration: f32 = trajectory_file.segments.iter().map(|s| s.duration).sum();
    let total_bytes = trajectory_file.segments.len() * 132;

    println!("Summary:");
    println!("  Segments: {}", trajectory_file.segments.len());
    println!("  Total duration: {:.2}s", total_duration);
    println!("  Memory required: {} bytes", total_bytes);
    println!();

    // Calculate bounding box
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    let mut min_z = f32::MAX;
    let mut max_z = f32::MIN;

    for seg in &trajectory_file.segments {
        // The first coefficient (x^0) is the starting position
        min_x = min_x.min(seg.x[0]);
        max_x = max_x.max(seg.x[0]);
        min_y = min_y.min(seg.y[0]);
        max_y = max_y.max(seg.y[0]);
        min_z = min_z.min(seg.z[0]);
        max_z = max_z.max(seg.z[0]);
    }

    println!("Approximate bounding box (from starting positions):");
    println!("  X: {:.3}m to {:.3}m (range: {:.3}m)", min_x, max_x, max_x - min_x);
    println!("  Y: {:.3}m to {:.3}m (range: {:.3}m)", min_y, max_y, max_y - min_y);
    println!("  Z: {:.3}m to {:.3}m (range: {:.3}m)", min_z, max_z, max_z - min_z);
    println!();

    println!("Segments:");
    let mut cumulative_time = 0.0f32;
    for (i, seg) in trajectory_file.segments.iter().enumerate() {
        println!(
            "  [{:2}] t={:.2}s-{:.2}s (dur={:.2}s) start=({:.3}, {:.3}, {:.3})",
            i + 1,
            cumulative_time,
            cumulative_time + seg.duration,
            seg.duration,
            seg.x[0],
            seg.y[0],
            seg.z[0]
        );
        cumulative_time += seg.duration;
    }

    Ok(())
}

/// Display trajectory memory information from the Crazyflie
pub async fn display_memory(cf: &Crazyflie) -> Result<()> {
    let memories = cf.memory.get_memories(Some(MemoryType::Trajectory));

    if memories.is_empty() {
        println!("No trajectory memory found on Crazyflie");
        return Ok(());
    }

    println!("Trajectory Memory Information:");
    println!("==============================");
    println!();

    for mem in memories {
        println!("Memory ID: {}", mem.memory_id);
        println!("  Type: {:?}", mem.memory_type);
        println!("  Size: {} bytes ({:.1} KB)", mem.size, mem.size as f32 / 1024.0);

        // Calculate how many Poly4D segments can fit
        let max_segments = mem.size / 132;
        println!("  Max Poly4D segments: {}", max_segments);

        // Estimate max trajectory duration (assuming ~1s per segment average)
        println!("  Estimated max duration: ~{}s (at 1s/segment)", max_segments);
        println!();
    }

    println!("Note: Trajectory definitions (ID -> memory mappings) are stored in RAM");
    println!("      and cannot be queried. Use 'cfcli trajectory upload' to define trajectories.");

    Ok(())
}
