//! `cfcli swarm`: stored swarms, the selected swarm, and which of its
//! Crazyflies answer.
//!
//! Swarms are files in the Swarmkeeper format (see [`store`]). One of them is
//! selected and stored in the cfcli config; `--swarm` picks another one for a
//! single command. Crazyflies are named by their short `name`, which is also
//! what `--cf`/`--exclude` and `select --from-swarm` take.

pub mod store;

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_lib::crazyflie_link::LinkContext;
use inquire::validator::Validation;
use inquire::{Confirm, MultiSelect, Select, Text};
use tabled::Tabled;

use crate::error::CliError;
use crate::utils::display::{csv_row, print_table, table};
use crate::utils::radio::{self, RadioUri};
use crate::{
    Config, SwarmAddParameters, SwarmCommands, SwarmConfigCommands, SwarmCreateParameters,
    SwarmExportParameters, SwarmImportParameters, SwarmTargetArgs,
};
use store::{Store, Swarm, Unit};

/// Attempts at reaching a Crazyflie in `swarm scan` before calling it offline.
const SCAN_ATTEMPTS: usize = 3;

pub(crate) async fn run(
    config: &mut Config,
    command: &SwarmCommands,
    link_context: &LinkContext,
    non_interactive: bool,
    csv: bool,
) -> Result<()> {
    let store = Store::open()?;
    match command {
        SwarmCommands::Config { command } => match command {
            SwarmConfigCommands::List => list(&store, config, csv),
            SwarmConfigCommands::Select { id } => select(&store, config, id.as_deref(), non_interactive, csv),
            SwarmConfigCommands::Create(params) => create(&store, config, params),
            SwarmConfigCommands::Delete { id } => delete(&store, config, id.as_deref(), non_interactive),
            SwarmConfigCommands::Show { id } => show(&store, &swarm_id(config, id.as_deref())?, csv),
            SwarmConfigCommands::Add(params) => {
                let id = swarm_id(config, params.swarm.as_deref())?;
                add(&store, config, &id, params, link_context, non_interactive).await
            }
            SwarmConfigCommands::Remove { names, swarm } => {
                remove(&store, &swarm_id(config, swarm.as_deref())?, names, non_interactive)
            }
            SwarmConfigCommands::Rename { current, new_name, swarm } => rename(
                &store,
                &swarm_id(config, swarm.as_deref())?,
                current.as_deref(),
                new_name.as_deref(),
                non_interactive,
            ),
            SwarmConfigCommands::Import(params) => import(&store, config, params),
            SwarmConfigCommands::Export(params) => export(&store, &swarm_id(config, params.id.as_deref())?, params),
        },
        SwarmCommands::Scan(target) => {
            let (swarm, selected) = target_units(&store, config, target)?;
            scan(link_context, &swarm, &selected, csv).await
        }
    }
}

/// The swarm and the Crazyflies in it that a command contacts.
fn target_units(store: &Store, config: &Config, target: &SwarmTargetArgs) -> Result<(Swarm, Vec<usize>)> {
    let id = swarm_id(config, target.swarm.as_deref())?;
    let swarm = store.load(&id)?;
    let selected = swarm.select(&id, &target.cf, &target.exclude)?;
    Ok((swarm, selected))
}

/// The swarm a command acts on: the one given, else the selected one.
fn swarm_id(config: &Config, given: Option<&str>) -> Result<String> {
    match given.or(config.swarm.as_deref()) {
        Some(id) => Ok(id.to_string()),
        None => bail!(CliError::NotFound(
            "selected swarm; pick one with 'cfcli swarm config select'".to_string()
        )),
    }
}

fn save_config(config: &Config) {
    confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
        println!("Could not save configuration: {:?}", err);
    });
}

fn set_selected(config: &mut Config, id: &str, swarm: &Swarm) {
    config.swarm = Some(id.to_string());
    save_config(config);
    println!("Selected swarm '{}' ({})", id, crazyflies(swarm.units.len()));
}

/// Select `id` when no (existing) swarm is selected, so the first swarm a
/// user creates or imports is ready to use.
fn select_if_none(store: &Store, config: &mut Config, id: &str, swarm: &Swarm) {
    if !config.swarm.as_deref().is_some_and(|selected| store.exists(selected)) {
        set_selected(config, id, swarm);
    }
}

fn crazyflies(n: usize) -> String {
    match n {
        1 => "1 Crazyflie".to_string(),
        n => format!("{} Crazyflies", n),
    }
}

/// One row of `swarm config list`.
#[derive(Tabled)]
struct SwarmRow {
    #[tabled(rename = "")]
    selected: String,
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Crazyflies")]
    count: String,
}

fn list(store: &Store, config: &Config, csv: bool) -> Result<()> {
    let ids = store.ids()?;
    let rows: Vec<SwarmRow> = ids
        .iter()
        .map(|id| {
            let (name, count) = match store.load(id) {
                Ok(swarm) => (swarm.name, swarm.units.len().to_string()),
                Err(e) => (format!("(can't read: {:#})", e), "?".to_string()),
            };
            let selected = if config.swarm.as_deref() == Some(id.as_str()) { "*" } else { "" };
            SwarmRow { selected: selected.to_string(), id: id.clone(), name, count }
        })
        .collect();

    if csv {
        println!("id,name,crazyflies,selected");
        for row in &rows {
            let selected = if row.selected.is_empty() { "no" } else { "yes" };
            csv_row(&[&row.id, &row.name, &row.count, selected]);
        }
    } else if rows.is_empty() {
        println!(
            "No swarms yet. Create one with 'cfcli swarm config create <id>' or import a \
             Swarmkeeper file with 'cfcli swarm config import <file>'."
        );
    } else {
        print_table(&table(&rows));
    }
    Ok(())
}

fn select(store: &Store, config: &mut Config, id: Option<&str>, non_interactive: bool, csv: bool) -> Result<()> {
    let id = match id {
        Some(id) => id.to_string(),
        // "Get a list or set via name": without a TTY there is nothing to
        // pick from, so show what could be selected.
        None if non_interactive => return list(store, config, csv),
        None => pick_swarm(store, config, "Select a swarm:")?,
    };
    let swarm = store.load(&id)?;
    set_selected(config, &id, &swarm);
    Ok(())
}

/// Let the user pick a stored swarm, starting at the selected one.
fn pick_swarm(store: &Store, config: &Config, message: &str) -> Result<String> {
    let ids = store.ids()?;
    if ids.is_empty() {
        bail!(CliError::NotFound(
            "swarms; create one with 'cfcli swarm config create <id>'".to_string()
        ));
    }
    let labels: Vec<String> = ids
        .iter()
        .map(|id| match store.load(id) {
            Ok(swarm) => format!("{} - {} ({})", id, swarm.name, crazyflies(swarm.units.len())),
            Err(_) => format!("{} (can't read)", id),
        })
        .collect();
    let start = config
        .swarm
        .as_ref()
        .and_then(|selected| ids.iter().position(|id| id == selected))
        .unwrap_or(0);
    let picked = Select::new(message, labels)
        .with_starting_cursor(start)
        .raw_prompt()
        .map_err(|_| anyhow!("No swarm selected"))?;
    Ok(ids[picked.index].clone())
}

fn create(store: &Store, config: &mut Config, params: &SwarmCreateParameters) -> Result<()> {
    store::check_id(&params.id)?;
    if store.exists(&params.id) {
        bail!(CliError::InvalidValue(format!("swarm '{}' already exists", params.id)));
    }
    let swarm = Swarm::new(params.name.clone().unwrap_or_else(|| params.id.clone()), params.description.clone());
    store.save(&params.id, &swarm)?;
    println!("Created swarm '{}'", params.id);
    if params.select {
        set_selected(config, &params.id, &swarm);
    } else {
        select_if_none(store, config, &params.id, &swarm);
    }
    Ok(())
}

fn delete(store: &Store, config: &mut Config, id: Option<&str>, non_interactive: bool) -> Result<()> {
    let id = match id {
        Some(id) => id.to_string(),
        None => {
            crate::require_arg(non_interactive, "<SWARM>")?;
            pick_swarm(store, config, "Select the swarm to delete:")?
        }
    };
    // Deleting can't be undone, so ask whenever there is someone to ask.
    if !non_interactive {
        let what = match store.load(&id) {
            Ok(swarm) => format!("swarm '{}' ({})", id, crazyflies(swarm.units.len())),
            Err(_) => format!("swarm '{}'", id),
        };
        let confirmed = Confirm::new(&format!("Delete {}?", what))
            .with_default(false)
            .prompt()
            .unwrap_or(false);
        if !confirmed {
            println!("Nothing deleted");
            return Ok(());
        }
    }
    store.delete(&id)?;
    println!("Deleted swarm '{}'", id);
    if config.swarm.as_deref() == Some(id.as_str()) {
        config.swarm = None;
        save_config(config);
        println!("No swarm is selected now");
    }
    Ok(())
}

/// One row of `swarm config show`.
#[derive(Tabled)]
struct UnitRow {
    #[tabled(rename = "CF")]
    name: String,
    #[tabled(rename = "URI")]
    uri: String,
    #[tabled(rename = "Description")]
    description: String,
}

fn show(store: &Store, id: &str, csv: bool) -> Result<()> {
    let swarm = store.load(id)?;
    if csv {
        println!("cf,uri,description");
        for unit in &swarm.units {
            csv_row(&[&unit.name, &unit.uri, unit.description.as_deref().unwrap_or_default()]);
        }
        return Ok(());
    }

    println!("Swarm '{}': {}", id, swarm.name);
    if let Some(description) = &swarm.description {
        println!("{}", description);
    }
    println!();
    if swarm.units.is_empty() {
        println!("No Crazyflies yet, add some with 'cfcli swarm config add'.");
    } else {
        let rows: Vec<UnitRow> = swarm
            .units
            .iter()
            .map(|unit| UnitRow {
                name: unit.name.clone(),
                uri: unit.uri.clone(),
                description: unit.description.clone().unwrap_or_default(),
            })
            .collect();
        print_table(&table(&rows));
    }
    Ok(())
}

/// The swarm `add` changes, checked to exist and be readable. Used by
/// `add --from-usb` in main.rs before it connects to anything.
pub(crate) fn swarm_to_change(config: &Config, given: Option<&str>) -> Result<String> {
    let id = swarm_id(config, given)?;
    Store::open()?.load(&id)?;
    Ok(id)
}

async fn add(
    store: &Store,
    config: &Config,
    id: &str,
    params: &SwarmAddParameters,
    link_context: &LinkContext,
    non_interactive: bool,
) -> Result<()> {
    let candidates: Vec<String> = match &params.scan {
        Some(address) => {
            let addresses = match address {
                Some(address) => vec![address.clone()],
                None => config.addresses.clone(),
            };
            let mut found = Vec::new();
            for address in &addresses {
                for uri in link_context.scan(crate::decode_address(address)?).await? {
                    // USB-attached Crazyflies show up in a scan too; a swarm
                    // is reached over the radio.
                    if uri.starts_with("radio://") && !found.contains(&uri) {
                        found.push(uri);
                    }
                }
            }
            if found.is_empty() {
                println!("No Crazyflies found on {}", addresses.join(", "));
                return Ok(());
            }
            found
        }
        None => params.uris.clone(),
    };
    add_to(store, id, params, candidates, non_interactive)
}

/// Add the Crazyflies on these URIs to swarm `id`. `add --from-usb` comes in
/// here from main.rs with the radio URI it read over USB.
pub(crate) fn add_uris(
    id: &str,
    params: &SwarmAddParameters,
    candidates: Vec<String>,
    non_interactive: bool,
) -> Result<()> {
    add_to(&Store::open()?, id, params, candidates, non_interactive)
}

fn add_to(
    store: &Store,
    id: &str,
    params: &SwarmAddParameters,
    candidates: Vec<String>,
    non_interactive: bool,
) -> Result<()> {
    if params.name.is_some() && candidates.len() > 1 {
        bail!(CliError::InvalidValue("--name can only be used when adding a single Crazyflie".to_string()));
    }
    let single = candidates.len() == 1;
    let mut swarm = store.load(id)?;
    let mut added = 0;
    for uri in candidates {
        store::check_uri(&uri)?;
        let (uri, _) = radio::any_radio(&uri);
        if let Some(i) = swarm.find_link(&uri)? {
            println!("{} is already in the swarm as {}", uri, swarm.units[i].name);
            continue;
        }
        let names: Vec<String> = swarm.units.iter().map(|u| u.name.clone()).collect();
        let name = match &params.name {
            Some(name) => {
                check_new_name(name, &names)?;
                name.clone()
            }
            // A single Crazyflie gets a name from the user, not a made-up one.
            None if single => {
                crate::require_arg(non_interactive, "--name")?;
                prompt_name(&format!("Name for {}:", uri), Some(&swarm.next_name()), names)?
            }
            None => swarm.next_name(),
        };
        println!("Added {} {}", name, uri);
        swarm.units.push(Unit {
            uri,
            name,
            description: params.description.clone(),
            extra: Default::default(),
        });
        added += 1;
    }

    if added > 0 {
        store.save(id, &swarm)?;
    }
    Ok(())
}

fn remove(store: &Store, id: &str, names: &[String], non_interactive: bool) -> Result<()> {
    let mut swarm = store.load(id)?;
    let mut indices = if names.is_empty() {
        crate::require_arg(non_interactive, "<CF>")?;
        pick_unit_indices(&swarm, id, "Select the Crazyflies to remove:")?
    } else {
        names.iter().map(|name| swarm.find_or_err(id, name)).collect::<Result<Vec<_>>>()?
    };
    if indices.is_empty() {
        println!("Nothing removed");
        return Ok(());
    }
    indices.sort_unstable();
    indices.dedup();
    for i in &indices {
        println!("Removed {} {}", swarm.units[*i].name, swarm.units[*i].uri);
    }
    for i in indices.iter().rev() {
        swarm.units.remove(*i);
    }
    store.save(id, &swarm)
}

fn rename(
    store: &Store,
    id: &str,
    current: Option<&str>,
    new_name: Option<&str>,
    non_interactive: bool,
) -> Result<()> {
    let mut swarm = store.load(id)?;
    let i = match current {
        Some(current) => swarm.find_or_err(id, current)?,
        None => {
            crate::require_arg(non_interactive, "<CF>")?;
            pick_unit_index(&swarm, id, "Select the Crazyflie to rename:")?
        }
    };
    // The others' names; keeping its own name (or changing its case) is fine.
    let others: Vec<String> = swarm
        .units
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != i)
        .map(|(_, u)| u.name.clone())
        .collect();
    let new_name = match new_name {
        Some(new_name) => {
            check_new_name(new_name, &others)?;
            new_name.to_string()
        }
        None => {
            crate::require_arg(non_interactive, "<NEW_NAME>")?;
            prompt_name(&format!("New name for {}:", swarm.units[i].name), None, others)?
        }
    };
    let old = std::mem::replace(&mut swarm.units[i].name, new_name.clone());
    store.save(id, &swarm)?;
    println!("Renamed {} to {}", old, new_name);
    Ok(())
}

/// Check a name for a Crazyflie against the names already in use.
fn check_new_name(name: &str, taken: &[String]) -> Result<()> {
    store::check_name(name)?;
    if let Some(other) = taken.iter().find(|t| t.eq_ignore_ascii_case(name)) {
        bail!(CliError::InvalidValue(format!("there is already a Crazyflie named '{}'", other)));
    }
    Ok(())
}

/// Ask for a Crazyflie name until it is usable and not in `taken`.
/// `default` is what an empty answer gives.
fn prompt_name(message: &str, default: Option<&str>, taken: Vec<String>) -> Result<String> {
    let validator = move |input: &str| {
        Ok(match check_new_name(input.trim(), &taken) {
            Ok(()) => Validation::Valid,
            // Shown under the prompt, where "invalid value:" adds nothing.
            Err(e) => Validation::Invalid(
                match e.downcast_ref::<CliError>() {
                    Some(CliError::InvalidValue(message)) => message.clone(),
                    _ => e.to_string(),
                }
                .into(),
            ),
        })
    };
    let mut prompt = Text::new(message).with_validator(validator);
    if let Some(default) = default {
        prompt = prompt.with_default(default);
    }
    let name = prompt.prompt().map_err(|_| anyhow!("No name entered"))?;
    Ok(name.trim().to_string())
}

/// The Crazyflies of a swarm as "name  URI" lines to pick from.
fn unit_labels(swarm: &Swarm, id: &str) -> Result<Vec<String>> {
    if swarm.units.is_empty() {
        bail!(CliError::NotFound(format!(
            "Crazyflies in swarm '{}' (add some with 'cfcli swarm config add')",
            id
        )));
    }
    let width = swarm.units.iter().map(|u| u.name.len()).max().unwrap_or(0);
    Ok(swarm
        .units
        .iter()
        .map(|u| format!("{:<width$}  {}", u.name, u.uri, width = width))
        .collect())
}

/// Let the user pick a Crazyflie.
fn pick_unit_index(swarm: &Swarm, id: &str, message: &str) -> Result<usize> {
    Ok(Select::new(message, unit_labels(swarm, id)?)
        .raw_prompt()
        .map_err(|_| anyhow!("No Crazyflie selected"))?
        .index)
}

/// Let the user pick any number of Crazyflies.
fn pick_unit_indices(swarm: &Swarm, id: &str, message: &str) -> Result<Vec<usize>> {
    Ok(MultiSelect::new(message, unit_labels(swarm, id)?)
        .raw_prompt()
        .map_err(|_| anyhow!("No Crazyflies selected"))?
        .into_iter()
        .map(|picked| picked.index)
        .collect())
}

fn import(store: &Store, config: &mut Config, params: &SwarmImportParameters) -> Result<()> {
    if params.id.is_some() && params.files.len() > 1 {
        bail!(CliError::InvalidValue("--id can only be used when importing a single file".to_string()));
    }

    // Read and check every file before writing any of them.
    let mut imports: Vec<(String, Swarm, &str)> = Vec::new();
    for file in &params.files {
        let yaml = match std::fs::read_to_string(file) {
            Ok(yaml) => yaml,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!(CliError::NotFound(format!("file '{}'", file)))
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", file)),
        };
        let swarm = Swarm::from_yaml(&yaml).with_context(|| format!("in {}", file))?;
        let id = match &params.id {
            Some(id) => id.clone(),
            None => std::path::Path::new(file)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        store::check_id(&id).with_context(|| format!("importing {} (pick another ID with --id)", file))?;
        if store.exists(&id) && !params.force {
            bail!(CliError::InvalidValue(format!(
                "swarm '{}' already exists (use --force to replace it)",
                id
            )));
        }
        if imports.iter().any(|(other, _, _)| *other == id) {
            bail!(CliError::InvalidValue(format!("two of the files would both become swarm '{}'", id)));
        }
        imports.push((id, swarm, file));
    }

    for (id, swarm, file) in &mut imports {
        let any_radio = swarm.use_any_radio();
        store.save(id, swarm)?;
        print!("Imported swarm '{}' from {} ({})", id, file, crazyflies(swarm.units.len()));
        if any_radio > 0 {
            print!(", {} URIs now use radio:/// (any Crazyradio)", any_radio);
        }
        println!();
    }
    if let Some((id, swarm, _)) = imports.first() {
        select_if_none(store, config, id, swarm);
    }
    Ok(())
}

fn export(store: &Store, id: &str, params: &SwarmExportParameters) -> Result<()> {
    let yaml = store.read_raw(id)?;
    match &params.output {
        Some(path) => {
            std::fs::write(path, yaml).with_context(|| format!("writing {}", path))?;
            println!("Exported swarm '{}' to {}", id, path);
        }
        None => print!("{}", yaml),
    }
    Ok(())
}

/// One row of `swarm scan`.
#[derive(Tabled)]
struct ScanRow {
    #[tabled(rename = "CF")]
    name: String,
    #[tabled(rename = "URI")]
    uri: String,
    #[tabled(rename = "Radio")]
    radio: String,
    #[tabled(rename = "Online")]
    online: String,
}

/// Check which of the selected Crazyflies answer. Each Crazyradio checks its
/// own Crazyflies, all radios at the same time.
async fn scan(link_context: &LinkContext, swarm: &Swarm, selected: &[usize], csv: bool) -> Result<()> {
    let units: Vec<&Unit> = selected.iter().map(|i| &swarm.units[*i]).collect();

    // Radio for each radio:// Crazyflie, None for usb://.
    let parsed = units.iter().map(|unit| RadioUri::parse(&unit.uri)).collect::<Result<Vec<_>>>()?;
    let links: Vec<radio::Link> = parsed
        .iter()
        .flatten()
        .map(|r| radio::Link { radio: r.radio, channel: r.channel })
        .collect();
    let mut radios: Vec<Option<usize>> = vec![None; units.len()];
    if !links.is_empty() {
        let available = radio::available_radios(link_context).await;
        if available.is_empty() {
            bail!(CliError::Connection("no Crazyradio could be opened".to_string()));
        }
        let assignment = radio::assign(&links, &available);
        for warning in &assignment.warnings {
            eprintln!("Warning: {}", warning);
        }
        let mut assigned = assignment.radios.into_iter();
        for (radio, parsed) in radios.iter_mut().zip(&parsed) {
            if parsed.is_some() {
                *radio = assigned.next();
            }
        }
    }

    let usb_present = if parsed.iter().any(|p| p.is_none()) {
        crate::scan_usb(link_context).await?
    } else {
        Vec::new()
    };

    let mut by_radio: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for (i, radio) in radios.iter().enumerate() {
        if let Some(radio) = radio {
            by_radio.entry(*radio).or_default().push(i);
        }
    }
    let probes = by_radio.into_iter().map(|(radio, members)| {
        let parsed = &parsed;
        async move {
            let mut results = Vec::new();
            for i in members {
                let uri = parsed[i].as_ref().expect("only radio URIs get a radio").with_radio(radio);
                let mut online = false;
                for _ in 0..SCAN_ATTEMPTS {
                    if matches!(link_context.scan_selected(vec![uri.as_str()]).await, Ok(found) if !found.is_empty()) {
                        online = true;
                        break;
                    }
                }
                results.push((i, online));
            }
            results
        }
    });
    let mut online = vec![false; units.len()];
    for (i, answered) in futures::future::join_all(probes).await.into_iter().flatten() {
        online[i] = answered;
    }
    for (i, unit) in units.iter().enumerate() {
        if parsed[i].is_none() {
            online[i] = usb_present.contains(&unit.uri);
        }
    }

    let rows: Vec<ScanRow> = units
        .iter()
        .enumerate()
        .map(|(i, unit)| ScanRow {
            name: unit.name.clone(),
            uri: unit.uri.clone(),
            radio: radios[i].map(|r| r.to_string()).unwrap_or_else(|| "USB".to_string()),
            online: if online[i] { "yes" } else { "no" }.to_string(),
        })
        .collect();

    if csv {
        println!("cf,uri,radio,online");
        for row in &rows {
            csv_row(&[&row.name, &row.uri, &row.radio, &row.online]);
        }
    } else {
        print_table(&table(&rows));
        let answered = online.iter().filter(|o| **o).count();
        println!("{} of {} answered", answered, crazyflies(units.len()));
    }
    Ok(())
}

/// The Crazyflie `cfcli select --from-swarm` picks from the selected swarm,
/// by name or interactively. Returns its URI as written in the swarm (an
/// empty radio is filled in at connect time) and a description for the user.
pub fn pick_unit(config: &Config, query: Option<&str>, non_interactive: bool) -> Result<(String, String)> {
    let store = Store::open()?;
    let id = swarm_id(config, None)?;
    let swarm = store.load(&id)?;
    let i = match query {
        Some(query) => swarm.find_or_err(&id, query)?,
        None => {
            crate::require_arg(non_interactive, "--from-swarm <CF>")?;
            pick_unit_index(&swarm, &id, "Select a Crazyflie:")?
        }
    };
    let unit = &swarm.units[i];
    Ok((unit.uri.clone(), format!("{} in swarm '{}'", unit.name, id)))
}
