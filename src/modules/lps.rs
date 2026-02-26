use anyhow::{bail, Result};
use crazyflie_lib::{
    subsystems::memory::{LocoMemory2, MemoryType},
    Crazyflie,
};

pub async fn display(cf: &Crazyflie) -> Result<()> {
    let memories = cf.memory.get_memories(Some(MemoryType::Loco2));

    if memories.is_empty() {
        bail!("No Loco Positioning v2 memory found. Is the LPS deck attached?");
    }

    let loco_mem = match cf
        .memory
        .open_memory::<LocoMemory2>(memories[0].clone())
        .await
    {
        Some(Ok(m)) => m,
        Some(Err(e)) => bail!("Could not access Loco2 memory: {}", e),
        None => bail!("Loco2 memory not found"),
    };

    let data = loco_mem.read_all().await?;

    println!("Loco Positioning System - Anchor Data:");
    println!("  {:>3}  {:>6}  {:>5}  {}", "ID", "Active", "Valid", "Position (x, y, z)");

    for &id in &data.anchor_ids {
        let is_active = data.active_anchor_ids.contains(&id);
        if let Some(anchor) = data.anchors.get(&id) {
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

    cf.memory.close_memory(loco_mem).await?;

    Ok(())
}
