use anyhow::Result;
use crate::{Config, console_preserve_path, decode_address};
use crate::modules::swarm::store::Store;

pub fn show(config: &Config) {
    println!("Timeout: {}ms{}", config.effective_timeout(),
        if config.timeout_ms.is_none() { " (default)" } else { "" });
    println!("Scan addresses:");
    for addr in &config.addresses {
        println!("  {}", addr);
    }
    println!("Preserved console history file: {}", console_preserve_path().display());
    println!("Selected swarm: {}", config.swarm.as_deref().unwrap_or("none"));
    sync_show(config);
    if let Ok(store) = Store::open() {
        println!("Swarm folder: {}", store.dir().display());
    }
}

pub fn timeout_show(config: &Config) {
    println!("Timeout: {}ms{}", config.effective_timeout(),
        if config.timeout_ms.is_none() { " (default)" } else { "" });
}

pub fn timeout_set(config: &mut Config, timeout_ms: u32) {
    config.timeout_ms = Some(timeout_ms);
    confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
        println!("Could not save settings: {:?}", err);
    });
    println!("Timeout set to {}ms", timeout_ms);
}

pub fn timeout_clear(config: &mut Config) {
    config.timeout_ms = None;
    confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
        println!("Could not save settings: {:?}", err);
    });
    println!("Timeout reset to default (1000ms)");
}

pub fn address_list(config: &Config) {
    println!("Scan addresses:");
    for addr in &config.addresses {
        println!("  {}", addr);
    }
}

pub fn address_add(config: &mut Config, address: &str) -> Result<()> {
    decode_address(address)?;
    let normalized = address.to_uppercase();
    if config.addresses.contains(&normalized) {
        println!("Address {} is already in the list", normalized);
    } else {
        config.addresses.push(normalized.clone());
        confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
            println!("Could not save settings: {:?}", err);
        });
        println!("Added address {}", normalized);
    }
    Ok(())
}

pub fn address_remove(config: &mut Config, address: &str) {
    let normalized = address.to_uppercase();
    if let Some(pos) = config.addresses.iter().position(|a| a == &normalized) {
        config.addresses.remove(pos);
        if config.addresses.is_empty() {
            config.addresses.push("E7E7E7E7E7".to_string());
            println!("Removed address {}. List was empty, reset to default (E7E7E7E7E7)", normalized);
        } else {
            println!("Removed address {}", normalized);
        }
        confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
            println!("Could not save settings: {:?}", err);
        });
    } else {
        println!("Address {} not found in the list", normalized);
    }
}

pub fn address_clear(config: &mut Config) {
    config.addresses = vec!["E7E7E7E7E7".to_string()];
    confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
        println!("Could not save settings: {:?}", err);
    });
    println!("Addresses reset to default (E7E7E7E7E7)");
}

pub fn sync_show(config: &Config) {
    println!(
        "Sync shared swarms: {}{}",
        if config.sync_on() { "on" } else { "off" },
        if config.sync.is_none() { " (default)" } else { "" }
    );
}

pub fn sync_set(config: &mut Config, on: bool) {
    config.sync = Some(on);
    confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
        println!("Could not save settings: {:?}", err);
    });
    if on {
        println!("Sync is on: swarm commands check the server first and upload changes right away");
    } else {
        println!(
            "Sync is off: swarm commands use this computer's copies of shared swarms; \
             'cfcli swarm config pull' and 'push' sync them"
        );
    }
}
