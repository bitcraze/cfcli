//! Stored lighthouse configurations: `lh config list`, `save`, `import`,
//! `export`, `delete`, `move`, `pull` and `push`, and the configuration
//! that `write` and `check` use.
//!
//! Like swarms, local configurations are files in a `lighthouse` folder next
//! to the cfcli config, named `<config>`, and shared ones are on the server,
//! named `<org>/<config>` (see [`crate::modules::documents`]). The files are
//! the Crazyflie client's format, so `export` gives a file it opens. A file
//! may also have a `name`, which the client ignores.

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_lib::Crazyflie;
use inquire::{Confirm, Select};
use tabled::Tabled;

use super::{compare, describe_distance, CalibrationDelta, LighthouseConfigFile, Part};
use crate::error::CliError;
use crate::modules::documents::{Document, Documents, SharedId};
use crate::modules::swarm::Swarms;
use crate::utils::display::{csv_row, print_table, table};
use crate::Config;

impl Document for LighthouseConfigFile {
    const FOLDER: &'static str = "lighthouse";
    const NOUN: &'static str = "lighthouse config";
    const COMMAND: &'static str = "lh config";
    const SERVER_COUNT: &'static str = "base_stations";

    fn from_yaml(yaml: &str) -> Result<Self> {
        LighthouseConfigFile::from_yaml(yaml)
    }

    fn to_yaml(&self) -> Result<String> {
        LighthouseConfigFile::to_yaml(self)
    }

    /// The base stations with a position: a Crazyflie also keeps the
    /// calibration of base stations it has seen elsewhere.
    fn summary(&self) -> (String, usize) {
        (self.name().unwrap_or_default().to_string(), self.geos.len())
    }
}

impl LighthouseConfigFile {
    /// The `name` in the file, if it has one.
    pub fn name(&self) -> Option<&str> {
        self.extra.get("name").and_then(|v| v.as_str()).filter(|name| !name.trim().is_empty())
    }

    pub fn set_name(&mut self, name: &str) {
        self.extra.insert("name".into(), name.into());
    }
}

/// Local and shared lighthouse configurations.
pub(crate) type LhConfigs = Documents<LighthouseConfigFile>;

fn base_stations(n: usize) -> String {
    match n {
        1 => "1 base station".to_string(),
        n => format!("{} base stations", n),
    }
}

/// A stored configuration for the user: `'lab/cage' (revision 3)`.
fn describe(configs: &LhConfigs, id: &str) -> String {
    let revision = SharedId::parse(id)
        .ok()
        .flatten()
        .and_then(|shared| configs.shared(&shared).ok()?.copy_state(&shared).ok()?)
        .map(|copy| copy.revision)
        .filter(|revision| *revision > 0);
    match revision {
        Some(revision) => format!("'{}' (revision {})", id, revision),
        None => format!("'{}'", id),
    }
}

// ---- Which configuration a command uses ----

/// The configuration for `write` and `check`, and how to name it: a stored
/// one (`id`), a file (`input`), what is piped in, or else the one the
/// selected swarm names.
pub async fn source(config: &Config, id: Option<&str>, input: Option<&str>) -> Result<(LighthouseConfigFile, String)> {
    use std::io::IsTerminal;
    if let Some(id) = id {
        let configs = LhConfigs::open(config)?;
        return Ok((configs.load(id).await?, describe(&configs, id)));
    }
    if input.is_some() || !std::io::stdin().is_terminal() {
        let file = super::load(input)?;
        return Ok((file, input.unwrap_or("the configuration from stdin").to_string()));
    }
    let linked = match &config.swarm {
        Some(swarm) => Swarms::open(config)?.load(swarm).await?.lighthouse,
        None => None,
    };
    match linked {
        Some(id) => {
            let configs = LhConfigs::open(config)?;
            let file = configs.load(&id).await?;
            let label = describe(&configs, &id);
            println!("Using lighthouse config {}, which the selected swarm flies in", label);
            Ok((file, label))
        }
        None => bail!(CliError::MissingArg(
            "a lighthouse configuration: give a stored one's ID, a file with -i, or pipe one in \
             (the selected swarm names none)"
                .to_string()
        )),
    }
}

// ---- Listing and showing ----

/// One row of `lh config list`.
#[derive(Tabled)]
struct ConfigRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Base stations")]
    count: String,
    #[tabled(rename = "Stored")]
    stored: String,
}

pub async fn list(configs: &LhConfigs, csv: bool) -> Result<()> {
    let entries = configs.entries().await?;
    if csv {
        csv_row(&["id", "name", "base_stations", "stored", "revision", "not_pushed"]);
        for entry in &entries {
            let revision = entry.revision.map(|r| r.to_string()).unwrap_or_default();
            csv_row(&[
                &entry.id,
                &entry.name,
                &entry.count,
                &entry.place,
                &revision,
                if entry.pending { "yes" } else { "no" },
            ]);
        }
    } else if entries.is_empty() {
        println!(
            "No lighthouse configs yet. Store a Crazyflie's with 'cfcli lh config save <id>', or \
             import a file from the Crazyflie client with 'cfcli lh config import <file>'."
        );
    } else {
        let rows: Vec<ConfigRow> = entries
            .iter()
            .map(|entry| ConfigRow {
                id: entry.id.clone(),
                name: entry.name.clone(),
                count: entry.count.clone(),
                stored: entry.stored(),
            })
            .collect();
        print_table(&table(&rows));
        if configs.shared.as_ref().is_some_and(|shared| !shared.sync()) && entries.iter().any(|e| e.revision.is_some()) {
            println!("Sync is off: shared configs are this computer's copies ('cfcli lh config pull' gets the latest)");
        }
    }
    Ok(())
}

/// `lh config display <id>`
pub async fn show(configs: &LhConfigs, id: &str, csv: bool) -> Result<()> {
    let file = configs.load(id).await?;
    let title = match file.name() {
        Some(name) => format!("Lighthouse Configuration {}: {}", describe(configs, id), name),
        None => format!("Lighthouse Configuration {}", describe(configs, id)),
    };
    super::print_config(&file, &title, csv);
    Ok(())
}

/// Let the user pick a stored configuration.
async fn pick(configs: &LhConfigs, message: &str) -> Result<String> {
    let entries = configs.entries().await?;
    if entries.is_empty() {
        bail!(CliError::NotFound(
            "lighthouse configs; store one with 'cfcli lh config save <id>'".to_string()
        ));
    }
    let labels: Vec<String> = entries
        .iter()
        .map(|e| match e.name.is_empty() {
            true => format!("{} ({} base stations)", e.id, e.count),
            false => format!("{} - {} ({} base stations)", e.id, e.name, e.count),
        })
        .collect();
    let picked = Select::new(message, labels)
        .raw_prompt()
        .map_err(|_| anyhow!("No lighthouse config selected"))?;
    Ok(entries[picked.index].id.clone())
}

// ---- Storing ----

/// What changes for each base station when `old` becomes `new`.
pub fn changes(old: &LighthouseConfigFile, new: &LighthouseConfigFile) -> Vec<String> {
    let mut changes = Vec::new();
    for diff in compare(new, old) {
        let bs = format!("BS {} (channel {})", diff.id, diff.id + 1);
        match diff.geometry {
            Part::Differs(d) => changes.push(format!(
                "{}: moved {}, turned {:.2}°",
                bs,
                describe_distance(d.moved_m),
                d.turned_deg
            )),
            Part::OnlyInFile => changes.push(format!("{}: position added", bs)),
            Part::OnlyOnCf => changes.push(format!("{}: position removed", bs)),
            Part::Same | Part::Absent => {}
        }
        match diff.calibration {
            Part::Differs(CalibrationDelta::Replaced { file_uid, cf_uid }) => changes.push(format!(
                "{}: another base station, 0x{:08X} instead of 0x{:08X}",
                bs, file_uid, cf_uid
            )),
            Part::Differs(CalibrationDelta::Values { .. }) => changes.push(format!("{}: calibration changed", bs)),
            Part::OnlyInFile => changes.push(format!("{}: calibration added", bs)),
            Part::OnlyOnCf => changes.push(format!("{}: calibration removed", bs)),
            Part::Same | Part::Absent => {}
        }
    }
    if old.name() != new.name() {
        changes.push(format!("name: {}", new.name().unwrap_or("(none)")));
    }
    changes
}

/// Store `new` as `id`: create it, or replace what is there after showing
/// what changes and asking (unless `force`). A shared configuration keeps
/// the old one as an earlier revision.
async fn store(configs: &LhConfigs, id: &str, mut new: LighthouseConfigFile, force: bool, non_interactive: bool) -> Result<()> {
    if new.geos.is_empty() {
        bail!(CliError::InvalidValue(
            "the configuration has no base station positions (geometry); estimate it first, \
             e.g. in the Crazyflie client's Lighthouse tab"
                .to_string()
        ));
    }
    let existing = match configs.load(id).await {
        Ok(old) => Some(old),
        Err(e) if matches!(e.downcast_ref::<CliError>(), Some(CliError::NotFound(_))) => None,
        Err(e) => return Err(e),
    };
    let Some(old) = existing else {
        configs.create(id, &new).await?;
        println!(
            "Stored lighthouse config {} on {}: {}",
            describe(configs, id),
            configs.place(id),
            base_stations(new.geos.len())
        );
        return Ok(());
    };

    // What the new configuration doesn't say, like its name, stays.
    for (key, value) in &old.extra {
        if !new.extra.contains_key(key) {
            new.extra.insert(key.clone(), value.clone());
        }
    }
    let changes = changes(&old, &new);
    if changes.is_empty() {
        println!("Lighthouse config {} is already this configuration", describe(configs, id));
        return Ok(());
    }
    println!("Changes to lighthouse config {}:", describe(configs, id));
    for change in &changes {
        println!("  {}", change);
    }
    if !force {
        crate::require_arg(non_interactive, "--force")?;
        let question = match SharedId::parse(id)? {
            Some(shared) => format!("Update '{}' on {}, for everyone in {}?", id, configs.place(id), shared.org),
            None => format!("Replace '{}'?", id),
        };
        if !Confirm::new(&question).with_default(false).prompt().unwrap_or(false) {
            println!("Nothing changed");
            return Ok(());
        }
    }
    configs
        .change(id, |file| {
            *file = new.clone();
            Ok(())
        })
        .await?;
    println!("Updated lighthouse config {}", describe(configs, id));
    Ok(())
}

/// `lh config save <id>`: the Crazyflie's configuration.
pub async fn save(
    configs: &LhConfigs,
    cf: &Crazyflie,
    id: &str,
    name: Option<&str>,
    force: bool,
    non_interactive: bool,
) -> Result<()> {
    check_new_id(configs, id)?;
    let mut file = super::read_with_progress(cf, non_interactive).await?;
    if let Some(name) = name {
        file.set_name(name);
    }
    store(configs, id, file, force, non_interactive).await
}

/// `lh config import <file>`: a file from the Crazyflie client (or `read`).
pub async fn import(
    configs: &LhConfigs,
    path: &str,
    id: Option<&str>,
    name: Option<&str>,
    force: bool,
    non_interactive: bool,
) -> Result<()> {
    let id = match id {
        Some(id) => id.to_string(),
        None => std::path::Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    check_new_id(configs, &id).with_context(|| format!("importing {} (pick another ID with --id)", path))?;
    let mut file = super::load(Some(path))?;
    if let Some(name) = name {
        file.set_name(name);
    }
    store(configs, &id, file, force, non_interactive).await
}

/// An ID to store under: a valid file name, and for a shared one, a sign-in.
fn check_new_id(configs: &LhConfigs, id: &str) -> Result<()> {
    match SharedId::parse(id)? {
        None => crate::modules::documents::check_id::<LighthouseConfigFile>(id),
        Some(shared) => configs.shared(&shared).map(|_| ()),
    }
}

/// `lh config export <id>`: the file, for the Crazyflie client.
pub async fn export(configs: &LhConfigs, id: &str, output: Option<&str>) -> Result<()> {
    let yaml = configs.load(id).await?.to_yaml()?;
    match output {
        Some(path) => {
            std::fs::write(path, yaml).with_context(|| format!("writing {}", path))?;
            println!("Exported lighthouse config {} to {}", describe(configs, id), path);
        }
        None => print!("{}", yaml),
    }
    Ok(())
}

// ---- Deleting and moving ----

pub async fn delete(configs: &LhConfigs, id: Option<&str>, non_interactive: bool) -> Result<()> {
    let id = match id {
        Some(id) => id.to_string(),
        None => {
            crate::require_arg(non_interactive, "<CONFIG>")?;
            pick(configs, "Select the lighthouse config to delete:").await?
        }
    };
    // Deleting can't be undone, so ask whenever there is someone to ask.
    if !non_interactive {
        let question = match SharedId::parse(&id)? {
            Some(shared) => format!(
                "Delete lighthouse config '{}' on {}, for everyone in {}?",
                id,
                configs.place(&id),
                shared.org
            ),
            None => format!("Delete lighthouse config '{}'?", id),
        };
        if !Confirm::new(&question).with_default(false).prompt().unwrap_or(false) {
            println!("Nothing deleted");
            return Ok(());
        }
    }
    configs.delete(&id).await?;
    println!("Deleted lighthouse config '{}'", id);
    Ok(())
}

/// `lh config move`: share one (cage -> org/cage), take it back
/// (org/cage -> cage), or rename it.
pub async fn move_config(configs: &LhConfigs, from: &str, to: &str, non_interactive: bool) -> Result<()> {
    if from == to {
        bail!(CliError::InvalidValue(format!("'{}' is already where it is", from)));
    }
    check_new_id(configs, to)?;
    let file = configs.load(from).await?;
    if SharedId::parse(to)?.is_none() && configs.local.exists(to) {
        bail!(CliError::InvalidValue(format!("lighthouse config '{}' already exists", to)));
    }
    // Moving a shared configuration away deletes it for everyone: ask.
    if let Some(shared) = SharedId::parse(from)? {
        if !non_interactive {
            let question = format!(
                "Move '{}' to '{}'? It is deleted on {} for everyone in {}.",
                from,
                to,
                configs.place(from),
                shared.org
            );
            if !Confirm::new(&question).with_default(false).prompt().unwrap_or(false) {
                println!("Nothing moved");
                return Ok(());
            }
        }
    }
    configs.create(to, &file).await?;
    if let Err(e) = configs.delete(from).await {
        bail!("copied '{}' to '{}', but '{}' is still there: {:#}", from, to, from, e);
    }
    println!(
        "Moved lighthouse config '{}' from {} to '{}' on {}",
        from,
        configs.place(from),
        to,
        configs.place(to)
    );
    println!("Swarms that name '{}' need 'cfcli swarm config lighthouse {}'", from, to);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
name: The cage
type: lighthouse_system_configuration
version: '1'
systemType: 2
geos:
  0:
    origin: [0.0, 0.0, 2.0]
    rotation: [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
calibs:
  0:
    uid: 2360210604
    sweeps:
    - {phase: 0.0, tilt: -0.051, curve: 0.275, gibmag: -0.005, gibphase: 2.281, ogeemag: -0.184, ogeephase: 1.847}
    - {phase: -0.004, tilt: 0.047, curve: 0.367, gibmag: -0.005, gibphase: 2.548, ogeemag: -0.124, ogeephase: 2.051}
";

    #[test]
    fn name_and_summary() {
        let mut file = LighthouseConfigFile::from_yaml(FILE).unwrap();
        assert_eq!(file.name(), Some("The cage"));
        assert_eq!(file.summary(), ("The cage".to_string(), 1));
        file.set_name("Fish tank");
        assert!(file.to_yaml().unwrap().contains("name: Fish tank"));
        // A file without positions counts no base stations.
        file.geos.clear();
        assert_eq!(file.summary().1, 0);
    }

    #[test]
    fn describes_changes() {
        let old = LighthouseConfigFile::from_yaml(FILE).unwrap();
        assert!(changes(&old, &old).is_empty());
        let mut new = old.clone();
        new.geos.get_mut(&0).unwrap().origin[0] = 0.05;
        new.calibs.get_mut(&0).unwrap().uid = 1;
        new.geos.insert(3, new.geos[&0].clone());
        let changes = changes(&old, &new);
        assert_eq!(
            changes,
            [
                "BS 0 (channel 1): moved 5.0 cm, turned 0.00°",
                "BS 0 (channel 1): another base station, 0x00000001 instead of 0x8CADF4AC",
                "BS 3 (channel 4): position added",
            ]
        );
    }
}
