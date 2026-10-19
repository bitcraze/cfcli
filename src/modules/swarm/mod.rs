//! `cfcli swarm`: stored swarms, the selected swarm, and which of its
//! Crazyflies answer.
//!
//! Local swarms are files in the Swarmkeeper format (see [`store`]), named
//! `<swarm>`. When cfcli is signed in (`cfcli auth login`), shared swarms on
//! the server sit next to them, named `<org>/<swarm>` (see
//! [`crate::modules::documents`]); every
//! command takes either. One swarm is selected and stored in the cfcli
//! config; `--swarm` picks another one for a single command. Crazyflies are
//! named by their short `name`, which is also what `--cf`/`--exclude` and
//! `select --from-swarm` take.

mod bootload;
mod broadcast;
mod commands;
mod lighthouse;
mod numbering;
mod rechannel;
mod reprogram;
mod runner;
pub mod store;

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_lib::crazyflie_link::LinkContext;
use inquire::validator::Validation;
use inquire::{Confirm, MultiSelect, Select, Text};
use tabled::Tabled;

use crate::error::CliError;
use crate::utils::connection::Connection;
use crate::utils::display::{csv_row, print_table, table};
use crate::utils::radio::{self, RadioUri};
use crate::{
    Config, ConfigTocCache, SwarmAddParameters, SwarmCommands, SwarmConfigCommands, SwarmCreateParameters,
    SwarmExportParameters, SwarmImportParameters, SwarmTargetArgs,
};
use runner::Runner;
use crate::modules::documents::{Documents, Entry, SharedId};
use numbering::Numbering;
use reprogram::Planned;
use store::{Swarm, Unit};

/// Local and shared swarms behind one kind of ID: `<swarm>` is a file in
/// the swarms folder, `<org>/<swarm>` a swarm on the server.
pub(crate) type Swarms = Documents<Swarm>;

pub(crate) async fn run(
    config: &mut Config,
    command: &SwarmCommands,
    connection: &mut Connection,
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
            SwarmConfigCommands::Name { name, swarm } => {
                swarm_name(&swarms, &swarm_id(config, swarm.as_deref())?, name.as_deref()).await
            }
            SwarmConfigCommands::Set { settings, swarm } => {
                set(&swarms, &swarm_id(config, swarm.as_deref())?, settings).await
            }
            SwarmConfigCommands::Clear { keys, swarm } => {
                clear(&swarms, &swarm_id(config, swarm.as_deref())?, keys).await
            }
            SwarmConfigCommands::Add(params) => {
                let id = swarm_id(config, params.swarm.as_deref())?;
                let client = connection.client().await?;
                add(&swarms, config, &id, params, client, link_context, toc_cache, non_interactive).await
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
            SwarmConfigCommands::Lh { config: new, clear, swarm } => {
                let id = swarm_id(config, swarm.as_deref())?;
                lighthouse::link(&swarms, config, &id, new.as_deref(), *clear, non_interactive).await
            }
            SwarmConfigCommands::Pull { id, force } => {
                let (shared, only) = swarms.for_sync(id.as_deref())?;
                shared.pull(only.as_ref(), *force).await
            }
            SwarmConfigCommands::Push { id, force } => {
                let (shared, only) = swarms.for_sync(id.as_deref())?;
                shared.push(only.as_ref(), *force).await
            }
        },
        SwarmCommands::Scan(target) => {
            scan(&runner(&swarms, config, target, connection, link_context).await?, csv).await
        }
        SwarmCommands::Platform { target, command } => {
            let runner = runner(&swarms, config, target, connection, link_context).await?;
            commands::platform(&runner, command, csv).await
        }
        SwarmCommands::Param { target, command } => {
            let runner = runner(&swarms, config, target, connection, link_context).await?;
            commands::param(&runner, command, non_interactive, csv).await
        }
        SwarmCommands::Deck { target, command } => {
            let runner = runner(&swarms, config, target, connection, link_context).await?;
            commands::deck(&runner, command, csv).await
        }
        SwarmCommands::Log { target, command } => {
            let runner = runner(&swarms, config, target, connection, link_context).await?;
            commands::log(&runner, command, non_interactive, csv).await
        }
        SwarmCommands::Bootload { target, command } => {
            let runner = runner(&swarms, config, target, connection, link_context).await?;
            bootload::bootload(&runner, command, toc_cache, non_interactive, csv).await
        }
        SwarmCommands::Debug { target, command } => {
            let runner = runner(&swarms, config, target, connection, link_context).await?;
            commands::debug(&runner, command, csv).await
        }
        SwarmCommands::Lh { target, command } => {
            let id = swarm_id(config, target.swarm.as_deref())?;
            let swarm = swarms.load(&id).await?;
            let selected = swarm.select(&id, &target.cf, &target.exclude)?;
            let wanted = lighthouse::wanted(config, &id, swarm.lighthouse.as_deref(), command, csv).await?;
            let client = connection.client().await?;
            let runner = Runner::new(client, link_context, config, &swarm, &selected).await?;
            lighthouse::run(&runner, &wanted, command, csv).await
        }
        SwarmCommands::Rechannel(params) => {
            let client = connection.client().await?;
            rechannel::rechannel(&swarms, config, params, client, link_context, non_interactive).await
        }
    }
}

/// A runner for the Crazyflies of the swarm that a command contacts.
async fn runner<'a>(
    swarms: &Swarms,
    config: &Config,
    target: &SwarmTargetArgs,
    connection: &mut Connection,
    link_context: &'a LinkContext,
) -> Result<Runner<'a>> {
    let id = swarm_id(config, target.swarm.as_deref())?;
    let swarm = swarms.load(&id).await?;
    let selected = swarm.select(&id, &target.cf, &target.exclude)?;
    Runner::new(connection.client().await?, link_context, config, &swarm, &selected).await
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
    let entries = swarms.entries().await?;
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
    let entries = swarms.entries().await?;
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
    // The organization only says where a shared swarm is, so it isn't part
    // of the name.
    let name = match (&params.name, SharedId::parse(&params.id)?) {
        (Some(name), _) => name.clone(),
        (None, Some(shared)) => shared.name,
        (None, None) => params.id.clone(),
    };
    let swarm = Swarm::new(name, params.description.clone());
    swarms.create(&params.id, &swarm).await?;
    println!("Created swarm '{}' on {}", params.id, swarms.place(&params.id));
    if params.select {
        set_selected(config, &params.id, &swarm);
    } else {
        select_if_none(swarms, config, &params.id, &swarm);
    }
    Ok(())
}

/// `swarm config name`: show the name the swarm is shown with, or set it.
async fn swarm_name(swarms: &Swarms, id: &str, name: Option<&str>) -> Result<()> {
    let Some(name) = name else {
        println!("{}", swarms.load(id).await?.name);
        return Ok(());
    };
    let name = name.trim();
    if name.is_empty() {
        bail!(CliError::InvalidValue("a swarm's name can't be empty".to_string()));
    }
    swarms
        .change(id, |swarm| {
            swarm.name = name.to_string();
            Ok(())
        })
        .await?;
    println!("Swarm '{}' is named '{}' now", id, name);
    Ok(())
}

/// The keys of `swarm config set` and `clear`.
const BASE_NAME: &str = "base-name";
const BASE_ADDRESS: &str = "base-address";

/// The swarm's base name and address as `swarm config show` shows them.
fn base_lines(swarm: &Swarm) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(base) = &swarm.base_name {
        let width = numbering::name_width(None, swarm);
        lines.push(format!("Base name: {} ({})", base, numbering::name_examples(base, width)));
    }
    if let Some(base) = &swarm.base_address {
        lines.push(format!("Base address: {} ({})", base, numbering::address_range(base)));
    }
    lines
}

/// `swarm config set key=value,...`: change the swarm's settings.
async fn set(swarms: &Swarms, id: &str, settings: &[String]) -> Result<()> {
    // Check every setting before changing anything.
    let mut base_name = None;
    let mut base_address = None;
    for pair in settings.iter().flat_map(|arg| arg.split(',')) {
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            CliError::InvalidValue(format!("'{}' isn't key=value, e.g. {}=CF-", pair.trim(), BASE_NAME))
        })?;
        let key = key.trim().replace('_', "-");
        if value.trim().is_empty() {
            bail!(CliError::InvalidValue(format!(
                "{} has no value; remove it with 'cfcli swarm config clear {}'",
                key, key
            )));
        }
        match key.as_str() {
            BASE_NAME => base_name = Some(numbering::check_base_name(value)?),
            BASE_ADDRESS => base_address = Some(numbering::check_base_address(value)?),
            _ => bail!(CliError::InvalidValue(format!(
                "unknown swarm setting '{}' (the settings are {} and {})",
                key, BASE_NAME, BASE_ADDRESS
            ))),
        }
    }
    let swarm = swarms
        .change(id, |swarm| {
            if let Some(base) = &base_name {
                swarm.base_name = Some(base.clone());
            }
            if let Some(base) = &base_address {
                swarm.base_address = Some(base.clone());
            }
            Ok(swarm.clone())
        })
        .await?;
    println!("Swarm '{}':", id);
    for line in base_lines(&swarm) {
        println!("{}", line);
    }
    Ok(())
}

/// `swarm config clear key ...`: remove some of the swarm's settings.
async fn clear(swarms: &Swarms, id: &str, keys: &[String]) -> Result<()> {
    swarms
        .change(id, |swarm| {
            for key in keys {
                match key.as_str() {
                    BASE_NAME => swarm.base_name = None,
                    BASE_ADDRESS => swarm.base_address = None,
                    _ => unreachable!("clap only lets the known keys through"),
                }
            }
            Ok(())
        })
        .await?;
    let cleared: Vec<String> = keys.iter().map(|key| key.replace('-', " ")).collect();
    println!("Swarm '{}' has no {} now", id, cleared.join(" or "));
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
    if let Some(lighthouse) = &swarm.lighthouse {
        println!("Flies in lighthouse config '{}'", lighthouse);
    }
    for line in base_lines(&swarm) {
        println!("{}", line);
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

#[allow(clippy::too_many_arguments)]
async fn add(
    swarms: &Swarms,
    config: &Config,
    id: &str,
    params: &SwarmAddParameters,
    client: &crazyflie_connection_server::Client,
    link_context: &LinkContext,
    toc_cache: ConfigTocCache,
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
                let address = crate::decode_address(address)?;
                let scanned = client.scan(Some(address)).await.context(CliError::Connection("scanning".to_string()))?;
                for uri in scanned.into_iter().map(|cf| cf.uri) {
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
    add_to(swarms, config, id, params, candidates, None, client, link_context, toc_cache, non_interactive).await
}

/// Add the Crazyflie found on USB (`usb`), whose config has the radio URI
/// `radio_uri`, to swarm `id`. `add --from-usb` comes in here from main.rs.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_from_usb(
    config: &Config,
    id: &str,
    params: &SwarmAddParameters,
    usb: &str,
    radio_uri: String,
    client: &crazyflie_connection_server::Client,
    link_context: &LinkContext,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
) -> Result<()> {
    let swarms = Swarms::open(config)?;
    add_to(&swarms, config, id, params, vec![radio_uri], Some(usb), client, link_context, toc_cache, non_interactive)
        .await
}

/// The base a `--base-name`/`--base-address` flag asks for: the one given,
/// else the swarm's. `None` when the flag isn't given.
fn base_from(
    flag: &Option<Option<String>>,
    stored: Option<&str>,
    id: &str,
    key: &str,
    example: &str,
    check: fn(&str) -> Result<String>,
) -> Result<Option<String>> {
    match flag {
        None => Ok(None),
        Some(Some(given)) => Ok(Some(check(given)?)),
        Some(None) => match stored {
            Some(stored) => Ok(Some(stored.to_string())),
            None => bail!(CliError::NotFound(format!(
                "{} of swarm '{}'; set one with 'cfcli swarm config set {}={}', or give one with --{}={}",
                key.replace('-', " "),
                id,
                key,
                example,
                key,
                example
            ))),
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn add_to(
    swarms: &Swarms,
    config: &Config,
    id: &str,
    params: &SwarmAddParameters,
    candidates: Vec<String>,
    usb: Option<&str>,
    client: &crazyflie_connection_server::Client,
    link_context: &LinkContext,
    toc_cache: ConfigTocCache,
    non_interactive: bool,
) -> Result<()> {
    if params.name.is_some() && candidates.len() > 1 {
        bail!(CliError::InvalidValue("--name can only be used when adding a single Crazyflie".to_string()));
    }
    if params.set_address.is_some() && candidates.len() > 1 {
        bail!(CliError::InvalidValue("--set-address can only be used when adding a single Crazyflie".to_string()));
    }
    let set_address = match &params.set_address {
        Some(address) => Some(crate::decode_address(address)?.iter().map(|b| format!("{:02X}", b)).collect::<String>()),
        None => None,
    };
    let reprogramming = set_address.is_some() || params.base_address.is_some() || params.set_channel.is_some();
    if reprogramming && !client.is_in_process() {
        bail!(CliError::Connection(
            "reprogramming (--base-address, --set-address, --set-channel) does not go through a connection server \
             yet (it writes the EEPROM). Use --connection direct"
                .to_string()
        ));
    }
    let mut uris = Vec::new();
    for uri in &candidates {
        store::check_uri(uri)?;
        if reprogramming && RadioUri::parse(uri)?.is_none() {
            bail!(CliError::InvalidValue(format!(
                "{} isn't on the radio, so it can't be reprogrammed (add a Crazyflie on USB with --from-usb)",
                uri
            )));
        }
        uris.push(radio::any_radio(uri).0);
    }

    let swarm = swarms.load(id).await?;
    let names: Vec<String> = swarm.units.iter().map(|u| u.name.clone()).collect();
    let base_name =
        base_from(&params.base_name, swarm.base_name.as_deref(), id, BASE_NAME, "CF-", numbering::check_base_name)?;
    let base_address = base_from(
        &params.base_address,
        swarm.base_address.as_deref(),
        id,
        BASE_ADDRESS,
        "ABAD1DEA",
        numbering::check_base_address,
    )?;
    if let Some(name) = &params.name {
        check_new_name(name, &names)?;
    }

    // Without --name or --base-name, the names come from the swarm's base
    // name (or CF-): as they are when adding several, as the suggestion
    // when asking for the name of one.
    let name_base = match (&params.name, base_name) {
        (Some(_), _) => None,
        (None, Some(base)) => Some(base),
        (None, None) => Some(swarm.base_name.clone().unwrap_or_else(|| numbering::DEFAULT_BASE_NAME.to_string())),
    };
    let mut numbering = Numbering::new(&swarm, name_base, base_address);
    if let Some(name) = &params.name {
        numbering.take_name(name);
    }

    // What each new Crazyflie is called and where it ends up, decided
    // before anything is changed.
    let mut messages = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();
    for uri in &uris {
        if let Some(i) = swarm.find_link(uri)? {
            messages.push(format!("{} is already in the swarm as {}", uri, swarm.units[i].name));
            continue;
        }
        if planned.iter().any(|p| store::same_link(&p.from, uri)) {
            continue;
        }
        let numbered = numbering.next()?;
        let new_uri = match RadioUri::parse(uri)? {
            Some(radio_uri) if reprogramming => {
                let channel = params.set_channel.unwrap_or(radio_uri.channel);
                let address = set_address.clone().or(numbered.address).unwrap_or_else(|| radio_uri.address.clone());
                radio_uri.with_link(channel, &address)
            }
            _ => uri.clone(),
        };
        if reprogramming {
            if let Some(i) = swarm.find_link(&new_uri)? {
                bail!(CliError::InvalidValue(format!(
                    "{} would be reprogrammed to {}, where {} already is",
                    uri, new_uri, swarm.units[i].name
                )));
            }
            if let Some(other) = planned.iter().find(|p| store::same_link(&p.uri, &new_uri)) {
                bail!(CliError::InvalidValue(format!(
                    "{} and {} would both be reprogrammed to {}",
                    other.from, uri, new_uri
                )));
            }
        }
        let name = params.name.clone().or(numbered.name).expect("a name is given or numbered");
        planned.push(Planned { from: uri.clone(), uri: new_uri, name });
    }

    if !planned.is_empty() {
        if reprogramming && radio::available_radios(link_context).await.is_empty() {
            bail!(CliError::Connection(
                "no Crazyradio could be opened; reprogramming needs one to reboot the Crazyflie and find it \
                 on its new address. Nothing was changed"
                    .to_string()
            ));
        }
        // A single Crazyflie gets its name from the user, unless --name or
        // --base-name names it. Ask before changing anything: a shared
        // swarm's change may run again.
        if let [one] = planned.as_mut_slice() {
            if params.name.is_none() && params.base_name.is_none() {
                crate::require_arg(non_interactive, "--name or --base-name")?;
                one.name = prompt_name(&format!("Name for {}:", one.uri), Some(&one.name), names)?;
            }
        }
    }

    // Reprogram the Crazyflies, and add the ones that answer on their new
    // address and channel.
    let mut results = Vec::new();
    for p in &planned {
        let result =
            if reprogramming {
                reprogram::reprogram(client, link_context, config, toc_cache.clone(), usb, p).await
            } else {
                Ok(())
            };
        if let Err(e) = &result {
            eprintln!("{}: not added: {:#}", p.name, e);
        }
        results.push(result);
    }
    let to_add: Vec<&Planned> =
        planned.iter().zip(&results).filter(|(_, result)| result.is_ok()).map(|(p, _)| p).collect();

    let (changed, added, lighthouse) = if to_add.is_empty() {
        (Vec::new(), Vec::new(), None)
    } else {
        swarms
            .change(id, |swarm| {
                let mut messages = Vec::new();
                let mut added = Vec::new();
                // Found by URI: on a newer version of a shared swarm
                // someone else may have added them already.
                for p in &to_add {
                    if let Some(i) = swarm.find_link(&p.uri)? {
                        messages.push(format!("{} is already in the swarm as {}", p.uri, swarm.units[i].name));
                        continue;
                    }
                    let names: Vec<String> = swarm.units.iter().map(|u| u.name.clone()).collect();
                    check_new_name(&p.name, &names)?;
                    messages.push(format!("Added {} {}", p.name, p.uri));
                    added.push(p.name.clone());
                    swarm.units.push(Unit {
                        uri: p.uri.clone(),
                        name: p.name.clone(),
                        description: params.description.clone(),
                        extra: Default::default(),
                    });
                }
                Ok((messages, added, swarm.lighthouse.clone()))
            })
            .await?
    };
    for message in messages.iter().chain(&changed) {
        println!("{}", message);
    }
    lighthouse::hint_for_new(id, lighthouse.as_deref(), &added);
    let (_, outcome) = runner::split(results);
    outcome.finish()
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
    let mut imports: Vec<(String, Swarm, &str, Option<String>)> = Vec::new();
    for file in &params.files {
        let yaml = match std::fs::read_to_string(file) {
            Ok(yaml) => yaml,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!(CliError::NotFound(format!("file '{}'", file)))
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", file)),
        };
        let mut swarm = Swarm::from_yaml(&yaml).with_context(|| format!("in {}", file))?;
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
        if imports.iter().any(|(other, _, _, _)| *other == id) {
            bail!(CliError::InvalidValue(format!("two of the files would both become swarm '{}'", id)));
        }
        let dropped = lighthouse::drop_unnameable(&id, &mut swarm);
        imports.push((id, swarm, file, dropped));
    }

    for (id, swarm, file, dropped) in &mut imports {
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
        if let Some(note) = dropped {
            println!("{}", note);
        }
    }
    if let Some((id, swarm, _, _)) = imports.first() {
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
    let mut swarm = swarms.load(from).await?;
    let dropped = lighthouse::drop_unnameable(to, &mut swarm);
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
    if let Some(note) = dropped {
        println!("{}", note);
    }
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
/// connection. The ones a connection server is connected to answer without
/// radio traffic.
async fn scan(runner: &Runner<'_>, csv: bool) -> Result<()> {
    let all: Vec<usize> = (0..runner.targets.len()).collect();
    let answered = runner.answering(&all).await?;
    let online: Vec<bool> = answered.iter().map(Option::is_some).collect();

    let rows: Vec<ScanRow> = runner
        .targets
        .iter()
        .zip(&answered)
        .map(|(t, answered)| ScanRow {
            name: t.name.clone(),
            uri: t.uri.clone(),
            // The radio it answered on, else (direct mode) the one it was given
            radio: match (t.is_radio(), answered.flatten().or(t.radio)) {
                (false, _) => "USB".to_string(),
                (true, Some(radio)) => radio.to_string(),
                (true, None) => String::new(),
            },
            online: if answered.is_some() { "yes" } else { "no" }.to_string(),
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
