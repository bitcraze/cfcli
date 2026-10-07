//! `cfcli swarm`: stored swarms, the selected swarm, and which of its
//! Crazyflies answer.
//!
//! Local swarms are files in the Swarmkeeper format (see [`store`]), named
//! `<swarm>`. When cfcli is signed in (`cfcli auth login`), shared swarms on
//! the server sit next to them, named `<org>/<swarm>` (see [`shared`]); every
//! command takes either. One swarm is selected and stored in the cfcli
//! config; `--swarm` picks another one for a single command. Crazyflies are
//! named by their short `name`, which is also what `--cf`/`--exclude` and
//! `select --from-swarm` take.

mod bootload;
mod commands;
mod rechannel;
mod runner;
pub mod shared;
pub mod store;

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_lib::crazyflie_link::LinkContext;
use inquire::validator::Validation;
use inquire::{Confirm, MultiSelect, Select, Text};
use tabled::Tabled;

use crate::error::CliError;
use crate::utils::display::{csv_row, print_table, table};
use crate::utils::radio;
use crate::{
    Config, ConfigTocCache, SwarmAddParameters, SwarmCommands, SwarmConfigCommands, SwarmCreateParameters,
    SwarmExportParameters, SwarmImportParameters, SwarmTargetArgs,
};
use runner::Runner;
use shared::{Shared, SharedId};
use store::{Store, Swarm, Unit};

/// Local and shared swarms behind one kind of ID: `<swarm>` is a file in
/// the swarms folder, `<org>/<swarm>` a swarm on the server.
pub(crate) struct Swarms {
    pub local: Store,
    /// None when cfcli isn't signed in.
    pub shared: Option<Shared>,
}

impl Swarms {
    pub fn open(config: &Config) -> Result<Self> {
        Ok(Swarms { local: Store::open()?, shared: Shared::open(config.sync_on())? })
    }

    /// For shell completion: only [`Swarms::cached`] and
    /// [`Swarms::cached_ids`] are used, which never ask the server.
    pub fn open_cached() -> Result<Self> {
        Ok(Swarms { local: Store::open()?, shared: Shared::open(false)? })
    }

    fn shared(&self, id: &SharedId) -> Result<&Shared> {
        self.shared.as_ref().ok_or_else(|| {
            CliError::NotFound(format!(
                "a sign-in: '{}' is a shared swarm, sign in with 'cfcli auth login'",
                id
            ))
            .into()
        })
    }

    pub async fn load(&self, id: &str) -> Result<Swarm> {
        match SharedId::parse(id)? {
            None => self.local.load(id),
            Some(shared) => self.shared(&shared)?.load(&shared).await,
        }
    }

    /// The swarm without asking the server (a shared one's copy), for shell
    /// completion and pickers.
    pub fn cached(&self, id: &str) -> Result<Swarm> {
        match SharedId::parse(id)? {
            None => self.local.load(id),
            Some(shared) => self.shared(&shared)?.cached(&shared),
        }
    }

    /// Change a swarm. `change` may run more than once for a shared swarm
    /// (see [`Shared::change`]), so it must not ask the user anything.
    pub async fn change<T>(&self, id: &str, mut change: impl FnMut(&mut Swarm) -> Result<T>) -> Result<T> {
        match SharedId::parse(id)? {
            None => {
                let mut swarm = self.local.load(id)?;
                let before = swarm.to_yaml()?;
                let result = change(&mut swarm)?;
                if swarm.to_yaml()? != before {
                    self.local.save(id, &swarm)?;
                }
                Ok(result)
            }
            Some(shared) => self.shared(&shared)?.change(&shared, change).await,
        }
    }

    /// Create a swarm. A shared one is created on the server.
    pub async fn create(&self, id: &str, swarm: &Swarm) -> Result<()> {
        match SharedId::parse(id)? {
            None => {
                store::check_id(id)?;
                if self.local.exists(id) {
                    bail!(CliError::InvalidValue(format!("swarm '{}' already exists", id)));
                }
                self.local.save(id, swarm)
            }
            Some(shared_id) => {
                let shared = self.shared(&shared_id)?;
                if !shared.create(&shared_id, swarm).await? {
                    bail!(CliError::InvalidValue(format!(
                        "swarm '{}' already exists on {}",
                        id,
                        shared.host()
                    )));
                }
                Ok(())
            }
        }
    }

    /// Delete a swarm. A shared one is deleted on the server, for everyone.
    pub async fn delete(&self, id: &str) -> Result<()> {
        match SharedId::parse(id)? {
            None => self.local.delete(id),
            Some(shared) => self.shared(&shared)?.delete(&shared).await,
        }
    }

    /// Local swarms and the shared ones there are copies of, without asking
    /// the server.
    pub fn cached_ids(&self) -> Result<Vec<String>> {
        let mut ids = self.local.ids()?;
        if let Some(shared) = &self.shared {
            ids.extend(shared.cached_ids().into_iter().map(|id| id.to_string()));
        }
        Ok(ids)
    }

    /// Where a swarm is kept, for the user.
    fn place(&self, id: &str) -> String {
        match (&self.shared, SharedId::parse(id)) {
            (Some(shared), Ok(Some(_))) => shared.host().to_string(),
            _ => "this computer".to_string(),
        }
    }
}

pub(crate) async fn run(
    config: &mut Config,
    command: &SwarmCommands,
    link_context: &LinkContext,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
    csv: bool,
) -> Result<()> {
    let swarms = Swarms::open(config)?;
    match command {
        SwarmCommands::Config { command } => match command {
            SwarmConfigCommands::List => list(&swarms, config, csv).await,
            SwarmConfigCommands::Select { id } => select(&swarms, config, id.as_deref(), non_interactive, csv).await,
            SwarmConfigCommands::Create(params) => create(&swarms, config, params).await,
            SwarmConfigCommands::Delete { id } => delete(&swarms, config, id.as_deref(), non_interactive).await,
            SwarmConfigCommands::Show { id } => show(&swarms, &swarm_id(config, id.as_deref())?, csv).await,
            SwarmConfigCommands::Add(params) => {
                let id = swarm_id(config, params.swarm.as_deref())?;
                add(&swarms, config, &id, params, link_context, non_interactive).await
            }
            SwarmConfigCommands::Remove { names, swarm } => {
                remove(&swarms, &swarm_id(config, swarm.as_deref())?, names, non_interactive).await
            }
            SwarmConfigCommands::Rename { current, new_name, swarm } => {
                rename(
                    &swarms,
                    &swarm_id(config, swarm.as_deref())?,
                    current.as_deref(),
                    new_name.as_deref(),
                    non_interactive,
                )
                .await
            }
            SwarmConfigCommands::Import(params) => import(&swarms, config, params).await,
            SwarmConfigCommands::Export(params) => {
                export(&swarms, &swarm_id(config, params.id.as_deref())?, params).await
            }
            SwarmConfigCommands::Move { from, to } => move_swarm(&swarms, config, from, to, non_interactive).await,
            SwarmConfigCommands::Pull { id, force } => {
                let (shared, only) = shared_for_sync(&swarms, id.as_deref())?;
                shared.pull(only.as_ref(), *force).await
            }
            SwarmConfigCommands::Push { id, force } => {
                let (shared, only) = shared_for_sync(&swarms, id.as_deref())?;
                shared.push(only.as_ref(), *force).await
            }
        },
        SwarmCommands::Scan(target) => {
            scan(&runner(&swarms, config, target, link_context, toc_cache).await?, csv).await
        }
        SwarmCommands::Platform { target, command } => {
            let runner = runner(&swarms, config, target, link_context, toc_cache).await?;
            commands::platform(&runner, command, csv).await
        }
        SwarmCommands::Param { target, command } => {
            let runner = runner(&swarms, config, target, link_context, toc_cache).await?;
            commands::param(&runner, command, non_interactive, csv).await
        }
        SwarmCommands::Deck { target, command } => {
            let runner = runner(&swarms, config, target, link_context, toc_cache).await?;
            commands::deck(&runner, command, csv).await
        }
        SwarmCommands::Log { target, command } => {
            let runner = runner(&swarms, config, target, link_context, toc_cache).await?;
            commands::log(&runner, command, non_interactive, csv).await
        }
        SwarmCommands::Bootload { target, command } => {
            let runner = runner(&swarms, config, target, link_context, toc_cache.clone()).await?;
            bootload::bootload(&runner, command, toc_cache, non_interactive, csv).await
        }
        SwarmCommands::Debug { target, command } => {
            let runner = runner(&swarms, config, target, link_context, toc_cache).await?;
            commands::debug(&runner, command, csv).await
        }
        SwarmCommands::Rechannel(params) => {
            rechannel::rechannel(&swarms, config, params, link_context, toc_cache, non_interactive).await
        }
    }
}

/// A runner for the Crazyflies of the swarm that a command contacts.
async fn runner<'a>(
    swarms: &Swarms,
    config: &Config,
    target: &SwarmTargetArgs,
    link_context: &'a LinkContext,
    toc_cache: ConfigTocCache,
) -> Result<Runner<'a>> {
    let id = swarm_id(config, target.swarm.as_deref())?;
    let swarm = swarms.load(&id).await?;
    let selected = swarm.select(&id, &target.cf, &target.exclude)?;
    Runner::new(link_context, config, toc_cache, &swarm, &selected).await
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

/// The server for `pull`/`push`, and the one shared swarm to sync if given.
fn shared_for_sync<'a>(swarms: &'a Swarms, id: Option<&str>) -> Result<(&'a Shared, Option<SharedId>)> {
    let Some(shared) = &swarms.shared else {
        bail!(CliError::NotFound(
            "a sign-in: shared swarms need 'cfcli auth login'".to_string()
        ));
    };
    let only = match id {
        None => None,
        Some(id) => Some(SharedId::parse(id)?.ok_or_else(|| {
            CliError::InvalidValue(format!(
                "'{}' is a local swarm; only shared swarms (<org>/<swarm>) are pulled and pushed",
                id
            ))
        })?),
    };
    Ok((shared, only))
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
/// user creates or imports is ready to use. A selected shared swarm is taken
/// to exist; checking would mean asking the server.
fn select_if_none(swarms: &Swarms, config: &mut Config, id: &str, swarm: &Swarm) {
    let selected_exists = config
        .swarm
        .as_deref()
        .is_some_and(|selected| selected.contains('/') || swarms.local.exists(selected));
    if !selected_exists {
        set_selected(config, id, swarm);
    }
}

fn crazyflies(n: usize) -> String {
    match n {
        1 => "1 Crazyflie".to_string(),
        n => format!("{} Crazyflies", n),
    }
}

/// A swarm as `list` and the pickers show it.
struct Entry {
    id: String,
    name: String,
    count: String,
    /// Where it is kept: "this computer" or the server.
    place: String,
    revision: Option<i32>,
    pending: bool,
}

impl Entry {
    fn stored(&self) -> String {
        match self.revision {
            None => self.place.clone(),
            Some(0) => format!("{}, not uploaded yet", self.place),
            Some(revision) if self.pending => format!("{}, revision {}, changes not pushed", self.place, revision),
            Some(revision) => format!("{}, revision {}", self.place, revision),
        }
    }
}

/// The local swarms, then the shared ones.
async fn entries(swarms: &Swarms) -> Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = swarms
        .local
        .ids()?
        .into_iter()
        .map(|id| {
            let (name, count) = match swarms.local.load(&id) {
                Ok(swarm) => (swarm.name, swarm.units.len().to_string()),
                Err(e) => (format!("(can't read: {:#})", e), "?".to_string()),
            };
            Entry { id, name, count, place: "this computer".to_string(), revision: None, pending: false }
        })
        .collect();
    if let Some(shared) = &swarms.shared {
        for listed in shared.list().await? {
            entries.push(Entry {
                id: listed.id.to_string(),
                name: listed.name,
                count: listed.units.to_string(),
                place: shared.host().to_string(),
                revision: Some(listed.copy.revision),
                pending: listed.copy.pending,
            });
        }
    }
    Ok(entries)
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
    #[tabled(rename = "Stored")]
    stored: String,
}

async fn list(swarms: &Swarms, config: &Config, csv: bool) -> Result<()> {
    let entries = entries(swarms).await?;
    let selected = |entry: &Entry| config.swarm.as_deref() == Some(entry.id.as_str());

    if csv {
        println!("id,name,crazyflies,selected,stored,revision,not_pushed");
        for entry in &entries {
            let revision = entry.revision.map(|r| r.to_string()).unwrap_or_default();
            csv_row(&[
                &entry.id,
                &entry.name,
                &entry.count,
                if selected(entry) { "yes" } else { "no" },
                &entry.place,
                &revision,
                if entry.pending { "yes" } else { "no" },
            ]);
        }
    } else if entries.is_empty() {
        println!(
            "No swarms yet. Create one with 'cfcli swarm config create <id>' or import a \
             Swarmkeeper file with 'cfcli swarm config import <file>'."
        );
    } else {
        let rows: Vec<SwarmRow> = entries
            .iter()
            .map(|entry| SwarmRow {
                selected: if selected(entry) { "*" } else { "" }.to_string(),
                id: entry.id.clone(),
                name: entry.name.clone(),
                count: entry.count.clone(),
                stored: entry.stored(),
            })
            .collect();
        print_table(&table(&rows));
        if swarms.shared.as_ref().is_some_and(|shared| !shared.sync()) && entries.iter().any(|e| e.revision.is_some()) {
            println!("Sync is off: shared swarms are this computer's copies ('cfcli swarm config pull' gets the latest)");
        }
    }
    Ok(())
}

async fn select(swarms: &Swarms, config: &mut Config, id: Option<&str>, non_interactive: bool, csv: bool) -> Result<()> {
    let id = match id {
        Some(id) => id.to_string(),
        // "Get a list or set via name": without a TTY there is nothing to
        // pick from, so show what could be selected.
        None if non_interactive => return list(swarms, config, csv).await,
        None => pick_swarm(swarms, config, "Select a swarm:").await?,
    };
    let swarm = swarms.load(&id).await?;
    set_selected(config, &id, &swarm);
    Ok(())
}

/// Let the user pick a swarm, starting at the selected one.
async fn pick_swarm(swarms: &Swarms, config: &Config, message: &str) -> Result<String> {
    let entries = entries(swarms).await?;
    if entries.is_empty() {
        bail!(CliError::NotFound(
            "swarms; create one with 'cfcli swarm config create <id>'".to_string()
        ));
    }
    let labels: Vec<String> = entries
        .iter()
        .map(|e| match e.count.parse::<usize>() {
            Ok(count) => format!("{} - {} ({})", e.id, e.name, crazyflies(count)),
            Err(_) => format!("{} (can't read)", e.id),
        })
        .collect();
    let start = config
        .swarm
        .as_ref()
        .and_then(|selected| entries.iter().position(|e| e.id == *selected))
        .unwrap_or(0);
    let picked = Select::new(message, labels)
        .with_starting_cursor(start)
        .raw_prompt()
        .map_err(|_| anyhow!("No swarm selected"))?;
    Ok(entries[picked.index].id.clone())
}

async fn create(swarms: &Swarms, config: &mut Config, params: &SwarmCreateParameters) -> Result<()> {
    let swarm = Swarm::new(params.name.clone().unwrap_or_else(|| params.id.clone()), params.description.clone());
    swarms.create(&params.id, &swarm).await?;
    println!("Created swarm '{}' on {}", params.id, swarms.place(&params.id));
    if params.select {
        set_selected(config, &params.id, &swarm);
    } else {
        select_if_none(swarms, config, &params.id, &swarm);
    }
    Ok(())
}

async fn delete(swarms: &Swarms, config: &mut Config, id: Option<&str>, non_interactive: bool) -> Result<()> {
    let id = match id {
        Some(id) => id.to_string(),
        None => {
            crate::require_arg(non_interactive, "<SWARM>")?;
            pick_swarm(swarms, config, "Select the swarm to delete:").await?
        }
    };
    // Deleting can't be undone, so ask whenever there is someone to ask.
    if !non_interactive {
        let what = match swarms.cached(&id) {
            Ok(swarm) => format!("swarm '{}' ({})", id, crazyflies(swarm.units.len())),
            Err(_) => format!("swarm '{}'", id),
        };
        let question = match SharedId::parse(&id)? {
            Some(shared) => format!(
                "Delete {} on {}, for everyone in {}?",
                what,
                swarms.place(&id),
                shared.org
            ),
            None => format!("Delete {}?", what),
        };
        let confirmed = Confirm::new(&question).with_default(false).prompt().unwrap_or(false);
        if !confirmed {
            println!("Nothing deleted");
            return Ok(());
        }
    }
    swarms.delete(&id).await?;
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

async fn show(swarms: &Swarms, id: &str, csv: bool) -> Result<()> {
    let swarm = swarms.load(id).await?;
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
    if SharedId::parse(id)?.is_some() {
        println!("Shared on {}", swarms.place(id));
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
pub(crate) async fn swarm_to_change(config: &Config, given: Option<&str>) -> Result<String> {
    let id = swarm_id(config, given)?;
    Swarms::open(config)?.load(&id).await?;
    Ok(id)
}

async fn add(
    swarms: &Swarms,
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
    add_to(swarms, id, params, candidates, non_interactive).await
}

/// Add the Crazyflies on these URIs to swarm `id`. `add --from-usb` comes in
/// here from main.rs with the radio URI it read over USB.
pub(crate) async fn add_uris(
    config: &Config,
    id: &str,
    params: &SwarmAddParameters,
    candidates: Vec<String>,
    non_interactive: bool,
) -> Result<()> {
    add_to(&Swarms::open(config)?, id, params, candidates, non_interactive).await
}

async fn add_to(
    swarms: &Swarms,
    id: &str,
    params: &SwarmAddParameters,
    candidates: Vec<String>,
    non_interactive: bool,
) -> Result<()> {
    if params.name.is_some() && candidates.len() > 1 {
        bail!(CliError::InvalidValue("--name can only be used when adding a single Crazyflie".to_string()));
    }
    let mut uris = Vec::new();
    for uri in &candidates {
        store::check_uri(uri)?;
        uris.push(radio::any_radio(uri).0);
    }

    // A single Crazyflie gets a name from the user, not a made-up one. Ask
    // before changing anything: a shared swarm's change may run again.
    let mut name = params.name.clone();
    if let [uri] = uris.as_slice() {
        let swarm = swarms.load(id).await?;
        let names: Vec<String> = swarm.units.iter().map(|u| u.name.clone()).collect();
        match &name {
            Some(name) => check_new_name(name, &names)?,
            None if swarm.find_link(uri)?.is_none() => {
                crate::require_arg(non_interactive, "--name")?;
                name = Some(prompt_name(&format!("Name for {}:", uri), Some(&swarm.next_name()), names)?);
            }
            None => {}
        }
    }

    let messages = swarms
        .change(id, |swarm| {
            let mut messages = Vec::new();
            for uri in &uris {
                if let Some(i) = swarm.find_link(uri)? {
                    messages.push(format!("{} is already in the swarm as {}", uri, swarm.units[i].name));
                    continue;
                }
                let name = match &name {
                    Some(name) => {
                        let names: Vec<String> = swarm.units.iter().map(|u| u.name.clone()).collect();
                        check_new_name(name, &names)?;
                        name.clone()
                    }
                    None => swarm.next_name(),
                };
                messages.push(format!("Added {} {}", name, uri));
                swarm.units.push(Unit {
                    uri: uri.clone(),
                    name,
                    description: params.description.clone(),
                    extra: Default::default(),
                });
            }
            Ok(messages)
        })
        .await?;
    for message in messages {
        println!("{}", message);
    }
    Ok(())
}

async fn remove(swarms: &Swarms, id: &str, names: &[String], non_interactive: bool) -> Result<()> {
    // Who to remove is decided on the swarm as it is now; the change finds
    // them again by name, in case it runs on a newer version.
    let swarm = swarms.load(id).await?;
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
    let gone: Vec<String> = indices.iter().map(|i| swarm.units[*i].name.clone()).collect();

    let messages = swarms
        .change(id, |swarm| {
            let mut indices: Vec<usize> = gone.iter().filter_map(|name| swarm.find(name)).collect();
            indices.sort_unstable();
            indices.dedup();
            let messages: Vec<String> = indices
                .iter()
                .map(|i| format!("Removed {} {}", swarm.units[*i].name, swarm.units[*i].uri))
                .collect();
            for i in indices.iter().rev() {
                swarm.units.remove(*i);
            }
            Ok(messages)
        })
        .await?;
    for message in messages {
        println!("{}", message);
    }
    Ok(())
}

async fn rename(
    swarms: &Swarms,
    id: &str,
    current: Option<&str>,
    new_name: Option<&str>,
    non_interactive: bool,
) -> Result<()> {
    let swarm = swarms.load(id).await?;
    let i = match current {
        Some(current) => swarm.find_or_err(id, current)?,
        None => {
            crate::require_arg(non_interactive, "<CF>")?;
            pick_unit_index(&swarm, id, "Select the Crazyflie to rename:")?
        }
    };
    let old = swarm.units[i].name.clone();
    // The others' names; keeping its own name (or changing its case) is fine.
    let others = |swarm: &Swarm, i: usize| -> Vec<String> {
        swarm
            .units
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, u)| u.name.clone())
            .collect()
    };
    let new_name = match new_name {
        Some(new_name) => {
            check_new_name(new_name, &others(&swarm, i))?;
            new_name.to_string()
        }
        None => {
            crate::require_arg(non_interactive, "<NEW_NAME>")?;
            prompt_name(&format!("New name for {}:", old), None, others(&swarm, i))?
        }
    };
    swarms
        .change(id, |swarm| {
            let i = swarm.find_or_err(id, &old)?;
            check_new_name(&new_name, &others(swarm, i))?;
            swarm.units[i].name = new_name.clone();
            Ok(())
        })
        .await?;
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

async fn import(swarms: &Swarms, config: &mut Config, params: &SwarmImportParameters) -> Result<()> {
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
        match SharedId::parse(&id).with_context(|| format!("importing {} (pick another ID with --id)", file))? {
            None => {
                store::check_id(&id).with_context(|| format!("importing {} (pick another ID with --id)", file))?;
                if swarms.local.exists(&id) && !params.force {
                    bail!(CliError::InvalidValue(format!(
                        "swarm '{}' already exists (use --force to replace it)",
                        id
                    )));
                }
            }
            Some(shared) => {
                swarms.shared(&shared)?;
            }
        }
        if imports.iter().any(|(other, _, _)| *other == id) {
            bail!(CliError::InvalidValue(format!("two of the files would both become swarm '{}'", id)));
        }
        imports.push((id, swarm, file));
    }

    for (id, swarm, file) in &mut imports {
        let any_radio = swarm.use_any_radio();
        match SharedId::parse(id)? {
            None => swarms.local.save(id, swarm)?,
            Some(shared_id) => {
                let shared = swarms.shared(&shared_id)?;
                if !shared.create(&shared_id, swarm).await? {
                    if !params.force {
                        bail!(CliError::InvalidValue(format!(
                            "swarm '{}' already exists on {} (use --force to replace it)",
                            id,
                            shared.host()
                        )));
                    }
                    let new = swarm.clone();
                    shared.change(&shared_id, |swarm| {
                        *swarm = new.clone();
                        Ok(())
                    })
                    .await?;
                }
            }
        }
        print!("Imported swarm '{}' from {} ({})", id, file, crazyflies(swarm.units.len()));
        if any_radio > 0 {
            print!(", {} URIs now use radio:/// (any Crazyradio)", any_radio);
        }
        println!();
    }
    if let Some((id, swarm, _)) = imports.first() {
        select_if_none(swarms, config, id, swarm);
    }
    Ok(())
}

async fn export(swarms: &Swarms, id: &str, params: &SwarmExportParameters) -> Result<()> {
    let yaml = match SharedId::parse(id)? {
        None => swarms.local.read_raw(id)?,
        Some(_) => swarms.load(id).await?.to_yaml()?,
    };
    match &params.output {
        Some(path) => {
            std::fs::write(path, yaml).with_context(|| format!("writing {}", path))?;
            println!("Exported swarm '{}' to {}", id, path);
        }
        None => print!("{}", yaml),
    }
    Ok(())
}

/// Move a swarm to a new ID: from this computer to the server
/// (`lab bitcraze/lab`), back (`bitcraze/lab lab`), or to another name. The
/// selection follows it.
async fn move_swarm(swarms: &Swarms, config: &mut Config, from: &str, to: &str, non_interactive: bool) -> Result<()> {
    if from == to {
        bail!(CliError::InvalidValue(format!("'{}' is already where it is", from)));
    }
    let from_shared = SharedId::parse(from)?;
    let to_shared = SharedId::parse(to)?;
    let swarm = swarms.load(from).await?;
    if to_shared.is_none() && swarms.local.exists(to) {
        bail!(CliError::InvalidValue(format!("swarm '{}' already exists", to)));
    }
    // Moving a shared swarm away deletes it for everyone: ask.
    if let Some(shared) = &from_shared {
        if !non_interactive {
            let question = format!(
                "Move '{}' to '{}'? It is deleted on {} for everyone in {}.",
                from,
                to,
                swarms.place(from),
                shared.org
            );
            if !Confirm::new(&question).with_default(false).prompt().unwrap_or(false) {
                println!("Nothing moved");
                return Ok(());
            }
        }
    }

    swarms.create(to, &swarm).await?;
    if let Err(e) = swarms.delete(from).await {
        bail!("copied '{}' to '{}', but '{}' is still there: {:#}", from, to, from, e);
    }
    println!(
        "Moved swarm '{}' ({}) from {} to '{}' on {}",
        from,
        crazyflies(swarm.units.len()),
        swarms.place(from),
        to,
        swarms.place(to)
    );
    if config.swarm.as_deref() == Some(from) {
        set_selected(config, to, &swarm);
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

/// Check which Crazyflies answer: one packet each (a few tries), no
/// connection.
async fn scan(runner: &Runner<'_>, csv: bool) -> Result<()> {
    let link_context = runner.link_context();
    let usb_present = if runner.targets.iter().any(|t| t.radio.is_none()) {
        crate::scan_usb(link_context).await?
    } else {
        Vec::new()
    };
    let results = runner
        .each("Checking", async |t| match t.radio {
            Some(_) => Ok(t.answers(link_context).await),
            None => Ok(usb_present.contains(&t.uri)),
        })
        .await;
    let online: Vec<bool> = results.into_iter().map(|r| r.unwrap_or(false)).collect();

    let rows: Vec<ScanRow> = runner
        .targets
        .iter()
        .zip(&online)
        .map(|(t, online)| ScanRow {
            name: t.name.clone(),
            uri: t.uri.clone(),
            radio: t.radio.map(|r| r.to_string()).unwrap_or_else(|| "USB".to_string()),
            online: if *online { "yes" } else { "no" }.to_string(),
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
        println!("{} of {} answered", answered, crazyflies(rows.len()));
    }
    Ok(())
}

/// The Crazyflie `cfcli select --from-swarm` picks from the selected swarm,
/// by name or interactively. Returns its URI as written in the swarm (an
/// empty radio is filled in at connect time) and a description for the user.
pub async fn pick_unit(config: &Config, query: Option<&str>, non_interactive: bool) -> Result<(String, String)> {
    let id = swarm_id(config, None)?;
    let swarm = Swarms::open(config)?.load(&id).await?;
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
