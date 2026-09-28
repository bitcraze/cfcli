use crate::modules::bootloader;
use crate::utils::deckctrl::DeckConfig;
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum, ValueHint};
use clap_num::maybe_hex;
use crazyflie_lib::subsystems::memory::{MemoryDevice, MemoryType, RadioSpeed, RawMemory};
use crazyflie_lib::TocCache;
use probe_rs::probe::list::Lister;
use probe_rs::{
    flashing::{DownloadOptions},
    Permissions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use inquire::Select;
use anyhow::{bail, Context, Result};
use tabled::settings::{object::Columns, Alignment, Modify};
use tabled::Tabled;

pub mod error;

pub mod modules {
    pub mod config;
    pub mod log;
    pub mod param;
    pub mod memory;
    pub mod bootloader;
    pub mod console;
    pub mod test;
    pub mod trajectory;
    pub mod lps;
    pub mod settings;
    pub mod crazyradio;
    pub mod debug;
    pub mod lighthouse;
    pub mod deck;
    pub mod platform;
    pub mod swarm;
}

pub mod utils {
    pub mod deckctrl;
    pub mod display;
    pub mod firmware;
    pub mod flash_source;
    pub mod radio;
}

use error::CliError;
use modules::settings;

include!("cli.rs");

/// One row of `mem list`.
#[derive(Tabled)]
struct MemoryRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Type")]
    memory_type: String,
    #[tabled(rename = "Size")]
    size: String,
    #[tabled(rename = "Serial")]
    serial: String,
}

impl MemoryTypeArg {
    /// Convert to the `crazyflie_lib` memory type. Defined here (not on the
    /// CLI type itself) so the CLI module stays free of the lib dependency.
    fn to_lib(self) -> MemoryType {
        match self {
            MemoryTypeArg::EepromConfig => MemoryType::EEPROMConfig,
            MemoryTypeArg::OneWire => MemoryType::OneWire,
            MemoryTypeArg::DriverLed => MemoryType::DriverLed,
            MemoryTypeArg::Loco => MemoryType::Loco,
            MemoryTypeArg::Trajectory => MemoryType::Trajectory,
            MemoryTypeArg::Loco2 => MemoryType::Loco2,
            MemoryTypeArg::Lighthouse => MemoryType::Lighthouse,
            MemoryTypeArg::MemoryTester => MemoryType::MemoryTester,
            MemoryTypeArg::MicroSd => MemoryType::MicroSD,
            MemoryTypeArg::DriverLedTiming => MemoryType::DriverLedTiming,
            MemoryTypeArg::App => MemoryType::App,
            MemoryTypeArg::DeckMemory => MemoryType::DeckMemory,
            MemoryTypeArg::DeckCtrlDfu => MemoryType::DeckCtrlDFU,
            MemoryTypeArg::DeckCtrl => MemoryType::DeckCtrl,
            MemoryTypeArg::DeckMultiranger => MemoryType::DeckMultiranger,
            MemoryTypeArg::DeckPaa3905 => MemoryType::DeckPaa3905,
        }
    }
}

fn resolve_memory_ref<'a>(
    memories: &[&'a MemoryDevice],
    reference: &MemoryRef,
) -> Result<&'a MemoryDevice> {
    match reference {
        MemoryRef::Id(id) => memories
            .iter()
            .find(|m| m.memory_id == *id)
            .copied()
            .ok_or_else(|| CliError::NotFound(format!("memory with ID {}", id)).into()),
        MemoryRef::Type(mt, instance) => {
            let lib_mt = mt.to_lib();
            let matches: Vec<&MemoryDevice> = memories
                .iter()
                .filter(|m| m.memory_type == lib_mt)
                .copied()
                .collect();
            match (matches.len(), instance) {
                (0, _) => bail!(CliError::NotFound(format!("memory of type {:?}", mt))),
                (_, Some(idx)) => match matches.get(*idx).copied() {
                    Some(m) => Ok(m),
                    None => bail!(CliError::InvalidValue(format!(
                        "instance {} of type {:?} out of range ({} found)",
                        idx, mt, matches.len()
                    ))),
                },
                (1, None) => Ok(matches[0]),
                (n, None) => bail!(CliError::InvalidValue(format!(
                    "multiple memories of type {:?} ({} found), specify an instance with '{:?}:0'..'{:?}:{}'",
                    mt, n, mt, mt, n - 1
                ))),
            }
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestCachedParameter {
    name: String,
    readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestCachedLogVariable {
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestCache {
    log: Vec<LatestCachedLogVariable>,
    param: Vec<LatestCachedParameter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    uri: String,
    toc_cache: HashMap<String, String>,
    #[serde(default)]
    timeout_ms: Option<u32>,
    #[serde(default = "default_addresses")]
    addresses: Vec<String>,
    /// ID of the selected swarm (see `cfcli swarm config select`)
    #[serde(default)]
    swarm: Option<String>,
}

fn default_addresses() -> Vec<String> {
    vec!["E7E7E7E7E7".to_string()]
}

impl Default for Config {
    fn default() -> Self {
        println!("No configuration found, loading default values");
        Config {
            uri: "".to_string(),
            toc_cache: HashMap::new(),
            timeout_ms: None,
            addresses: default_addresses(),
            swarm: None,
        }
    }
}

impl Config {
    fn effective_timeout(&self) -> u32 {
        self.timeout_ms.unwrap_or(1000)
    }
}

/// Add the connection timeout from the settings to a radio URI. It goes on
/// at connection time and is never stored in the selected URI.
fn with_timeout(config: &Config, uri: String) -> String {
    if config.timeout_ms.is_none() || uri.starts_with("usb://") {
        return uri;
    }
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{}{}timeout={}", uri, separator, config.effective_timeout())
}

#[derive(Clone)]
pub struct ConfigTocCache {
    config: Arc<std::sync::Mutex<Config>>,
    no_toc_cache: bool,
}

impl ConfigTocCache {
    fn new(config: Config, no_toc_cache: bool) -> Self {
        ConfigTocCache {
            config: Arc::new(std::sync::Mutex::new(config)),
            no_toc_cache,
        }
    }
}

impl TocCache for ConfigTocCache {
    fn get_toc(&self, crc32: &[u8]) -> Option<String> {
        match self.no_toc_cache {
            true => return None,
            false => self.config.lock().unwrap().toc_cache.get(&crc32.iter().map(|b| format!("{:02x}", b)).collect::<String>()).cloned(),
        } 
    }
    
    fn store_toc(&self, crc32: &[u8], toc: &str) {
        match self.no_toc_cache {
            true => return,
            false => {
              let mut config = self.config.lock().unwrap();
              config.toc_cache.insert(crc32.iter().map(|b| format!("{:02x}", b)).collect::<String>(), toc.to_string());
              confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
                  println!("Could not save configuration: {:?}", err);
              });              
            },
        }
    }
}

/// Bail with `CliError::MissingArg` when the caller would otherwise hit an
/// interactive `inquire` prompt but the CLI is running non-interactively
/// (`--non-interactive` set, or stdin is not a TTY). The message names the
/// missing argument so the caller knows which flag to add.
fn require_arg(non_interactive: bool, missing_arg: &str) -> Result<()> {
    if non_interactive {
        bail!(CliError::MissingArg(format!(
            "'{}' (running non-interactively)",
            missing_arg
        )));
    }
    Ok(())
}

/// Streaming commands are open-ended by design (live console, periodic log
/// stream, radio sniffer). When `--timeout` fires for one of these, that's
/// the user/agent's intended way to stop the stream, so we exit 0. Every
/// other command is bounded — a timeout means it got stuck, so we return
/// `CliError::Timeout` and exit 40.
fn is_streaming_command(cmd: &Commands) -> bool {
    match cmd {
        Commands::Log { command: LogCommands::Print(params) } => !params.once,
        Commands::Swarm { command: SwarmCommands::Log { command: SwarmLogCommands::Print(params), .. } } => {
            !params.once
        }
        Commands::Console { .. } | Commands::Cr { command: CrCommands::Sniff(_) } => true,
        _ => false,
    }
}

/// Whether a command talks to the selected (or `--uri`) Crazyflie. Only those
/// resolve a `radio:///` URI, which opens the Crazyradios to find a free one.
fn uses_selected_uri(cmd: &Commands) -> bool {
    !matches!(
        cmd,
        Commands::Completions { .. }
            | Commands::Scan(_)
            | Commands::Select(_)
            | Commands::Settings { .. }
            | Commands::Swarm { .. }
            | Commands::Cr { .. }
            | Commands::Util { .. }
            | Commands::Bootload { command: BootloadCommands::Releases | BootloadCommands::Targets }
    )
}

/// Connect to a Crazyflie and store the resulting handle in `holder` so the
/// centralized cleanup at the end of `run()` can disconnect it. Returns a
/// borrowed reference to the just-stored Crazyflie. The mutable borrow ends
/// when this function returns; callers can immediately use the `&Crazyflie`
/// alongside other immutable accesses to `holder`.
async fn connect_cf<'a>(
    holder: &'a mut Option<crazyflie_lib::Crazyflie>,
    link_context: &crazyflie_lib::crazyflie_link::LinkContext,
    uri: &str,
    toc_cache: ConfigTocCache,
    measure_connect_time: bool,
) -> Result<&'a crazyflie_lib::Crazyflie> {
    let start = if measure_connect_time { Some(std::time::Instant::now()) } else { None };
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache).await
        .context(CliError::Connection(format!("connecting to {}", uri)))?;
    if let Some(s) = start {
        eprintln!("Connection time: {:.2?}", s.elapsed());
    }
    let cf = holder.insert(cf);
    // Refresh the shell-completion cache with this drone's param/log names so
    // completion always reflects the most recently connected Crazyflie.
    update_completion_cache(cf).await;
    Ok(cf)
}

pub fn console_preserve_path() -> std::path::PathBuf {
    let config_path = confy::get_configuration_file_path("cf-cli", None)
        .expect("Could not determine config directory");
    config_path.with_file_name("cf-cli-console.log")
}

async fn save_console_history(cf: &crazyflie_lib::Crazyflie) -> Result<()> {
    use futures::StreamExt;
    use std::io::Write;

    let mut stream = cf.console.stream().await;
    if let Some(history) = stream.next().await {
        if !history.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(console_preserve_path())?;
            file.write_all(history.as_bytes())?;
        }
    }
    Ok(())
}

fn read_and_clear_console_file() -> Result<String> {
    let path = console_preserve_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    if !content.is_empty() {
        std::fs::write(&path, "")?;
    }
    Ok(content)
}

async fn save_and_disconnect(cf: &crazyflie_lib::Crazyflie, preserve_console: bool) {
    if preserve_console {
        if let Err(e) = save_console_history(cf).await {
            eprintln!("Warning: could not save console history: {}", e);
        }
    }
    cf.disconnect().await;
}

pub fn decode_address(address: &str) -> Result<[u8; 5]> {
    match u64::from_str_radix(&address.replace("0x", ""), 16) {
        Ok(a) if a <= 0xFFFFFFFFFF => Ok(a.to_be_bytes()[3..]
            .try_into()
            .expect("Could not convert u64 to [u8; 5]")),
        _ => bail!(CliError::InvalidValue(format!(
            "address '{}' is not a valid 5-byte hexadecimal value (e.g. E7E7E7E7E7)",
            address
        ))),
    }
}

/// List the URIs of all USB-attached Crazyflies. USB devices are found
/// regardless of radio address, so scanning on the default one is enough.
async fn scan_usb(link_context: &crazyflie_lib::crazyflie_link::LinkContext) -> Result<Vec<String>> {
    let address = decode_address("E7E7E7E7E7")?;
    let found = link_context.scan(address).await?;
    Ok(found.into_iter().filter(|uri| uri.starts_with("usb://")).collect())
}

/// Connect to a USB-attached Crazyflie, read the radio channel, speed and
/// address from its EEPROM config and return the matching radio URI. The
/// USB connection is closed again before returning.
async fn radio_uri_from_usb(
    holder: &mut Option<crazyflie_lib::Crazyflie>,
    link_context: &crazyflie_lib::crazyflie_link::LinkContext,
    usb_uri: &str,
    toc_cache: ConfigTocCache,
    measure_connect_time: bool,
    preserve_console: bool,
) -> Result<String> {
    let cf = connect_cf(holder, link_context, usb_uri, toc_cache, measure_connect_time).await?;
    let radio_uri = read_radio_uri(cf).await;
    save_and_disconnect(cf, preserve_console).await;
    holder.take();
    radio_uri
}

async fn read_radio_uri(cf: &crazyflie_lib::Crazyflie) -> Result<String> {
    let eeprom = modules::config::open(cf).await?;

    let speed_str = match eeprom.get_radio_speed() {
        RadioSpeed::R250Kbps => "250K",
        RadioSpeed::R1Mbps => "1M",
        RadioSpeed::R2Mbps => "2M",
    };
    let address_str = eeprom.get_radio_address().iter().map(|b| format!("{:02X}", b)).collect::<String>();

    Ok(format!("radio://0/{}/{}/{}", eeprom.get_radio_channel(), speed_str, address_str))
}

#[tokio::main]
async fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls aws-lc-rs CryptoProvider");

    let code = match run().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {:#}", e);
            if let Some(hint) = error::hint(&e) {
                eprintln!("Hint: {}", hint);
            }
            error::classify_exit_code(&e)
        }
    };
    std::process::exit(code);
}

// ---- Shell completion support -------------------------------------------

/// Path to the on-disk cache of the last-connected Crazyflie's parameter and
/// log variable names, used to feed dynamic shell completion. Lives next to
/// the confy config file.
fn completion_cache_path() -> std::path::PathBuf {
    let config_path = confy::get_configuration_file_path("cf-cli", None)
        .expect("Could not determine config directory");
    config_path.with_file_name("cf-cli-completion.json")
}

/// Overwrite the completion cache from a freshly-connected Crazyflie so that
/// completion always reflects the most recent drone. Best-effort: any error
/// is ignored so it can never break the command that triggered the connect.
async fn update_completion_cache(cf: &crazyflie_lib::Crazyflie) {
    let param = cf
        .param
        .names()
        .into_iter()
        .map(|name| {
            let readonly = !cf.param.is_writable(&name).unwrap_or(false);
            LatestCachedParameter { name, readonly }
        })
        .collect();
    let log = cf
        .log
        .names()
        .into_iter()
        .map(|name| LatestCachedLogVariable { name })
        .collect();
    let cache = LatestCache { log, param };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = std::fs::write(completion_cache_path(), json);
    }
}

/// Load the completion cache, returning an empty cache if it is missing or
/// unreadable (e.g. before the first successful connection).
fn load_completion_cache() -> LatestCache {
    std::fs::read_to_string(completion_cache_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(LatestCache { log: Vec::new(), param: Vec::new() })
}

/// Names of the Crazyflies in the selected swarm, for completion. Empty when
/// there is no config file yet: loading one would print a notice and create
/// it, which must not happen while the shell is completing.
fn selected_swarm_units() -> Vec<String> {
    let exists = confy::get_configuration_file_path("cf-cli", None).is_ok_and(|path| path.exists());
    let selected = exists
        .then(|| confy::load::<Config>("cf-cli", None).ok())
        .flatten()
        .and_then(|config| config.swarm);
    let Some(id) = selected else {
        return Vec::new();
    };
    modules::swarm::store::Store::open()
        .and_then(|store| store.load(&id))
        .map(|swarm| swarm.units.into_iter().map(|unit| unit.name).collect())
        .unwrap_or_default()
}

/// Print a shell completion script for `shell` to stdout.
fn emit_completion_script(shell: clap_complete::Shell) {
    let mut cmd = CliArgs::command();
    clap_complete::generate(shell, &mut cmd, "cfcli", &mut std::io::stdout());
}

/// Print dynamic completion candidates (one per line) for the `__complete`
/// helper. Reads only the local cache / hardcoded lists — never connects.
fn emit_dynamic_completions(kind: CompletionKind, partial: &str) {
    use std::io::Write;

    // Split a possibly comma-separated token into the committed prefix
    // (everything up to and including the last comma) and the fragment being
    // completed. Candidates are printed as `prefix + candidate` so the shell
    // replaces the whole word with a valid, fully-qualified token.
    let (prefix, fragment) = match partial.rfind(',') {
        Some(i) => (&partial[..=i], &partial[i + 1..]),
        None => ("", partial),
    };

    // In key=value lists (e.g. `--bin target=file.bin`) the text after '=' is
    // a file path, which the shell completes natively — not a name we know.
    if fragment.contains('=') {
        return;
    }

    let candidates: Vec<String> = match kind {
        CompletionKind::ParamNames => {
            load_completion_cache().param.into_iter().map(|p| p.name).collect()
        }
        CompletionKind::ParamNamesWritable => load_completion_cache()
            .param
            .into_iter()
            .filter(|p| !p.readonly)
            .map(|p| p.name)
            .collect(),
        CompletionKind::LogNames => {
            load_completion_cache().log.into_iter().map(|l| l.name).collect()
        }
        CompletionKind::FlashTargets => bootloader::get_hardcoded_list_of_targets()
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
        // Fixed set of EEPROM config keys accepted by `config set`. Keep in
        // sync with the ConfigNameAndValue doc comment and the run() handler.
        CompletionKind::ConfigKeys => ["channel", "address", "speed", "pitch_trim", "roll_trim"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        CompletionKind::SwarmConfigs => modules::swarm::store::Store::open()
            .and_then(|store| store.ids())
            .unwrap_or_default(),
        CompletionKind::SwarmUnits => selected_swarm_units(),
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for cand in candidates.iter().filter(|c| c.starts_with(fragment)) {
        let _ = writeln!(out, "{}{}", prefix, cand);
    }
}

async fn run() -> Result<()> {
    // The `__complete` dynamic-completion helper is dispatched before clap so
    // it never appears as a subcommand in `--help` or the generated completion
    // scripts. It is called only by the installed completion scripts, with
    // known arguments, so anything unparseable just emits no candidates.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).is_some_and(|a| a == "__complete") {
        if let Some(kind) = argv.get(2).and_then(|s| CompletionKind::from_str(s, false).ok()) {
            emit_dynamic_completions(kind, argv.get(3).map(String::as_str).unwrap_or(""));
        }
        return Ok(());
    }

    let args = CliArgs::parse();

    // The `completions` subcommand is handled up front: it never connects and
    // needs no config at all.
    if let Commands::Completions { shell } = &args.command {
        emit_completion_script(*shell);
        return Ok(());
    }

    let mut config: Config = confy::load("cf-cli", None).unwrap_or_else(|err| {
        println!("Could not load config file: {:?}", err);
        Config::default()
    });

    let toc_cache = ConfigTocCache::new(config.clone(), args.no_toc_cache);

    #[cfg(all(unix, feature = "packet_capture"))]
    crazyflie_lib::crazyflie_link::capture::init();

    let link_context = crazyflie_lib::crazyflie_link::LinkContext::new();

    let uri = {
        let base = args.uri.clone().unwrap_or(config.uri.clone());
        // A radio:/// URI (any Crazyradio) gets a real radio here, so every
        // command below can hand it straight to crazyflie-link.
        let base = if uses_selected_uri(&args.command) {
            utils::radio::resolve(&link_context, &base).await
        } else {
            base
        };
        with_timeout(&config, base)
    };

    let mut connected_cf: Option<crazyflie_lib::Crazyflie> = None;
    let preserve_console = args.preserve_console;
    let timeout_ms = args.timeout;
    let non_interactive = args.non_interactive || !std::io::stdin().is_terminal();
    let csv = args.csv;

    let body = async {
    match &args.command {
        // Handled before connection setup via early return in run().
        Commands::Completions { .. } => unreachable!(),
        Commands::Scan(scan_options) if scan_options.from_usb => {
            let mut found = Vec::new();
            for usb_uri in scan_usb(&link_context).await? {
                let radio_uri = radio_uri_from_usb(&mut connected_cf, &link_context, &usb_uri, toc_cache.clone(), args.debug, preserve_console).await?;
                found.push((usb_uri, radio_uri));
            }
            if csv {
                println!("usb_uri,radio_uri");
                for (usb_uri, radio_uri) in &found {
                    utils::display::csv_row(&[usb_uri, radio_uri]);
                }
            } else {
                for (usb_uri, radio_uri) in &found {
                    println!("> {} ({})", radio_uri, usb_uri);
                }
            }
        }
        Commands::Scan(scan_options) => {
            let addresses = match &scan_options.address {
                Some(addr) => vec![addr.clone()],
                None => config.addresses.clone(),
            };
            let mut found = Vec::new();
            for addr_str in &addresses {
                let address = decode_address(addr_str)?;
                for uri in link_context.scan(address).await? {
                    if !found.contains(&uri) {
                        found.push(uri);
                    }
                }
            }
            if csv {
                println!("uri");
                for uri in &found {
                    utils::display::csv_row(&[uri]);
                }
            } else {
                for uri in &found {
                    println!("> {}", uri);
                }
            }
        }
        Commands::Select(select_options) => {
            let mut selected_from = None;
            let selected_uri = if let Some(name) = &select_options.from_swarm {
                let (uri, from) = modules::swarm::pick_unit(&config, name.as_deref(), non_interactive)?;
                selected_from = Some(from);
                uri
            } else if select_options.from_usb {
                let found = scan_usb(&link_context).await?;

                if found.is_empty() {
                    bail!(CliError::Connection("no USB Crazyflies found".to_string()));
                }
                if found.len() != 1 {
                    bail!("Expected exactly one Crazyflie on USB, found {}", found.len());
                }

                let usb_uri = &found[0];
                println!("Found Crazyflie on USB: {}", usb_uri);

                radio_uri_from_usb(&mut connected_cf, &link_context, usb_uri, toc_cache, args.debug, preserve_console).await?
            } else {
                // Scan for Crazyflies on configured addresses
                let addresses = match &select_options.address {
                    Some(addr) => vec![addr.clone()],
                    None => config.addresses.clone(),
                };
                let mut found = Vec::new();
                for addr_str in &addresses {
                    let address = decode_address(addr_str)?;
                    for uri in link_context.scan(address).await? {
                        if !found.contains(&uri) {
                            found.push(uri);
                        }
                    }
                }

                if found.is_empty() {
                    bail!(CliError::Connection("no Crazyflies found on configured addresses".to_string()));
                }

                if select_options.auto {
                    if found.len() != 1 {
                        bail!("Expected exactly one Crazyflie, found {}", found.len());
                    }
                    found[0].clone()
                } else {
                    require_arg(non_interactive, "--auto")?;
                    Select::new("Select a link:", found.clone())
                        .prompt()
                        .map_err(|_| anyhow::anyhow!("No Crazyflie selected"))?
                }
            };

            match selected_from {
                Some(from) => println!("Selected: {} ({})", selected_uri, from),
                None => println!("Selected: {}", selected_uri),
            }
            config.uri = selected_uri.clone();

            confy::store("cf-cli", None, config).unwrap_or_else(|err| {
                println!("Could not save configuration: {:?}", err);
            });

        }
        Commands::Console { no_format, clear } => {
            if *clear {
                let path = console_preserve_path();
                if path.exists() {
                    std::fs::remove_file(&path)?;
                    println!("Cleared {}", path.display());
                } else {
                    println!("No preserved console history at {}", path.display());
                }
                return Ok(());
            }

            let saved = read_and_clear_console_file()?;
            if !saved.is_empty() {
                if *no_format {
                    print!("{}", saved);
                } else {
                    for line in saved.lines() {
                        print!("{}", modules::console::format_console_line(line));
                        println!();
                    }
                }
            }

            let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

            modules::console::print(cf, *no_format).await?;
            // Cleanup at end of run() handles disconnect.
        }
        Commands::Log { command } => {
            match command {
                LogCommands::List => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    modules::log::list(&cf, csv).await?;

                }
                LogCommands::Print(var) => {

                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    let names = match &var.names {
                      Some(n) => n.clone(),
                      None => {
                        require_arg(non_interactive, "<names>")?;
                        modules::log::pick_names(cf)?
                      }
                    };

                    if var.once {
                        modules::log::print_once(cf, &names, var.period as u64, csv).await?;
                    } else {
                        modules::log::print(cf, &names, var.period as u64, csv).await?;
                    }

                }
            }
        }
        Commands::Param { command } => {
            match command {
                ParamCommands::List => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::param::list(&cf, csv).await?;
                }
                ParamCommands::Get(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let names = match &var.names {
                      Some(n) => n.clone(),
                      None => {
                        require_arg(non_interactive, "<names>")?;
                        modules::param::pick_names(cf, "Select parameters to show:").await?
                      }
                    };

                    modules::param::get(&cf, &names, csv).await?;
                }
                ParamCommands::Set(params) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let param_list = match &params.params {
                      Some(p) => p.clone(),
                      None => {
                        require_arg(non_interactive, "<params>")?;
                        modules::param::pick_values(cf).await?
                      }
                    };

                    modules::param::set(&cf, &param_list, params.store).await?;
                }
                ParamCommands::Store(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let names = match &var.names {
                      Some(n) => n.clone(),
                      None => {
                        require_arg(non_interactive, "<names>")?;
                        modules::param::pick_persistent(cf, "Select parameters to store:").await?
                      }
                    };

                    modules::param::store(&cf, &names).await?;
                }
                ParamCommands::Clear(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let names = match &var.names {
                      Some(n) => n.clone(),
                      None => {
                        require_arg(non_interactive, "<names>")?;
                        modules::param::pick_persistent(cf, "Select parameters to clear:").await?
                      }
                    };

                    modules::param::clear(&cf, &names).await?;
                }
            }
        }
        Commands::Util { command } => {
            match command {
                UtilCommands::DeckCtrl { command } => {
                    match command {
                        DeckControlCommands::Bingen(params) => {
                            let deck_config = DeckConfig::from_yaml(params.input.clone())?;
                            if let Some(warning) = deck_config.name_warning() {
                                eprintln!("Warning: {}", warning);
                            }
                            let bytes = deck_config.to_bytes();
                            
                            if let Some(output) = &params.output {
                                std::fs::write(output, &bytes)?;
                            } else {
                                utils::display::hex_dump(bytes, 0);
                            }
                        }
                        DeckControlCommands::Binflash(params) => {
                            println!("Generating deck binary from {}", params.input);
                            let deck_config = DeckConfig::from_yaml(params.input.clone())?;
                            if let Some(warning) = deck_config.name_warning() {
                                eprintln!("Warning: {}", warning);
                            }
                            let bytes = deck_config.to_bytes();

                            let lister = Lister::new();
                            let probes = lister.list_all();

                            if probes.is_empty() {
                                bail!("No probes found, cannot flash deck");
                            }

                            let probe_idx = match params.probe_idx {
                                Some(idx) => {
                                  if idx < probes.len() {
                                    idx
                                  } else {
                                    bail!("Invalid probe index");
                                  }
                                },
                                None => {
                                    if probes.len() == 1 {
                                        0 as usize
                                    } else {
                                        require_arg(non_interactive, "--probe-idx")?;
                                        let options: Vec<String> = probes.iter().enumerate().map(|(i, p)| {
                                          format!("[{}] {} ({}:{}-{})", i, p.identifier, p.vendor_id, p.product_id, p.serial_number.as_deref().unwrap_or("N/A"))
                                        }).collect();

                                        let selected_option = Select::new("Select a probe:", options)
                                          .prompt()
                                          .map_err(|_| anyhow::anyhow!("No probe selected"))?;

                                        // Extract the probe index from the selected option
                                        let idx = selected_option
                                          .split(']')
                                          .next()
                                          .and_then(|s| s.trim_start_matches('[').parse::<usize>().ok())
                                          .ok_or_else(|| anyhow::anyhow!("Failed to parse probe index"))?;
                                        idx
                                    }
                                }
                            };

                            if probes.is_empty() {
                                bail!("No probes found, cannot flash deck");
                            }

                            let address = 0x08000000 + 1024 * 30;
                            let probe = probes[probe_idx].open()?;
                            let mut session =
                                probe.attach("STM32C011F6Ux", Permissions::default())?;

                            let mut loader = session.target().flash_loader();
                            loader.add_data(address, &bytes)?;
                            loader.commit(&mut session, DownloadOptions::default())?;

                            println!("Deck binary flashed successfully!");
                        }
                    }
                }
            }
        }
        Commands::Config { command } => {
            match command {
                ConfigCommands::Set(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let mut eeprom_memory = modules::config::open(cf).await?;

                    for (key, value) in &var.settings {
                      match key.as_str() {
                        // "name" => {
                        //   cf.platform.set_name(value).await?;
                        //   println!("Set platform name to {}", value);
                        // }
                        "channel" => {
                          let channel: u8 = match value.parse() {
                            Ok(c) if c <= 125 => c,
                            _ => bail!(CliError::InvalidValue(format!("channel '{}' must be an integer between 0 and 125", value))),
                          };
                          eeprom_memory.set_radio_channel(channel)?;
                          println!("Set radio channel to {}", channel);
                        }
                        "address" => {
                          let address: [u8; 5] = match u64::from_str_radix(&value.replace("0x", ""), 16) {
                            Ok(a) if a <= 0xFFFFFFFFFF => a.to_be_bytes()[3..]
                                .try_into()
                                .expect("Could not convert u64 to [u8; 5]"),
                            _ => bail!(CliError::InvalidValue(format!("address '{}' must be a 5-byte hexadecimal value (e.g. E7E7E7E7E7)", value))),
                          };
                          eeprom_memory.set_radio_address(address);
                          println!("Set radio address to {:02X?}", address);
                        }
                        "speed" => {
                          let speed: u8 = match value.parse() {
                            Ok(s) if s <= 2 => s,
                            _ => bail!(CliError::InvalidValue(format!("speed '{}' must be 0 (250Kbps), 1 (1Mbps) or 2 (2Mbps)", value))),
                          };
                          eeprom_memory.set_radio_speed(RadioSpeed::try_from(speed)?);
                          println!("Set radio speed to {}", speed);
                        }
                        "pitch_trim" => {
                          let pitch_trim: f32 = match value.parse() {
                            Ok(p) if p >= -20.0 && p <= 20.0 => p,
                            _ => bail!(CliError::InvalidValue(format!("pitch_trim '{}' must be a float between -20.0 and 20.0", value))),
                          };
                          eeprom_memory.set_pitch_trim(pitch_trim);
                          println!("Set pitch trim to {}", pitch_trim);
                        }
                        "roll_trim" => {
                          let roll_trim: f32 = match value.parse() {
                            Ok(r) if r >= -20.0 && r <= 20.0 => r,
                            _ => bail!(CliError::InvalidValue(format!("roll_trim '{}' must be a float between -20.0 and 20.0", value))),
                          };
                          eeprom_memory.set_roll_trim(roll_trim);
                          println!("Set roll trim to {}", roll_trim);
                        }
                        _ => {
                          println!("Unknown setting: {}", key);
                        }
                      }
                    }

                    eeprom_memory.commit().await?;

                }
                ConfigCommands::Display => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));

                    if memories.len() != 1 {
                      bail!("No EEPROMConfig memory found or more than one ({}), exiting!", memories.len());
                    }

                    modules::memory::display(&cf, memories[0].clone()).await?;

                  }
            }
        }
        Commands::Settings { command } => {
            match command {
                SettingsCommands::Show => settings::show(&config),
                SettingsCommands::Timeout { command: timeout_cmd } => {
                    match timeout_cmd {
                        SettingsTimeoutCommands::Show => settings::timeout_show(&config),
                        SettingsTimeoutCommands::Set { timeout_ms } => settings::timeout_set(&mut config, *timeout_ms),
                        SettingsTimeoutCommands::Clear => settings::timeout_clear(&mut config),
                    }
                }
                SettingsCommands::Address { command: addr_cmd } => {
                    match addr_cmd {
                        SettingsAddressCommands::List => settings::address_list(&config),
                        SettingsAddressCommands::Add { address } => settings::address_add(&mut config, address)?,
                        SettingsAddressCommands::Remove { address } => settings::address_remove(&mut config, address),
                        SettingsAddressCommands::Clear => settings::address_clear(&mut config),
                    }
                }
            }
        }
        Commands::Mem { command } => {
            match command {
                MemoryCommands::List => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memory = cf.memory.get_memories(None);

                    if csv {
                        println!("id,type,size_bytes,serial");
                        for mem in memory {
                            let serial = mem.serial.as_ref()
                                .map(|s| s.iter().map(|b| format!("{:02X}", b)).collect::<String>())
                                .unwrap_or_default();
                            utils::display::csv_row(&[
                                &mem.memory_id.to_string(),
                                &format!("{:?}", mem.memory_type),
                                &mem.size.to_string(),
                                &serial,
                            ]);
                        }
                    } else {
                        let rows: Vec<MemoryRow> = memory
                            .iter()
                            .map(|mem| MemoryRow {
                                id: mem.memory_id.to_string(),
                                memory_type: format!("{:?}", mem.memory_type),
                                size: format!("{} (0x{:x})", mem.size, mem.size),
                                serial: mem
                                    .serial
                                    .as_ref()
                                    .map(|s| s.iter().map(|b| format!("{:02X}", b)).collect::<String>())
                                    .unwrap_or_default(),
                            })
                            .collect();

                        // The ID is a short number, so right-aligning it keeps
                        // the column tidy next to the wider type names.
                        let mut table = utils::display::table(&rows);
                        table.with(Modify::new(Columns::one(0)).with(Alignment::right()));
                        utils::display::print_table(&table);
                    }


                }
                MemoryCommands::Read(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);
                    let device = resolve_memory_ref(&memories, &var.mem)?.clone();
                    let mem_id = device.memory_id;

                    let raw_access_memory = match cf.memory.open_memory::<RawMemory>(device).await {
                      Some(Ok(m)) => m,
                      Some(Err(e)) => bail!("Could not access memory ID={} as raw memory: {}", mem_id, e),
                      None => bail!("Memory ID={} not found", mem_id),
                    };

                    if let Some(output_file) = &var.output {

                        let progress_bar = utils::display::get_progressbar(var.length, None);
                        let pb = progress_bar.clone();
                        let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
                          pb.set_position(bytes_written as u64);
                        };
                        let data = raw_access_memory.read_with_progress(var.offset, var.length, progress_callback).await?;

                        utils::display::finish_progress(&progress_bar, format!("Read {} bytes from memory ID={} at offset 0x{:x}", var.length, mem_id, var.offset));

                      std::fs::write(output_file, &data)?;
                    } else {
                      let data = raw_access_memory.read(var.offset, var.length).await?;
                      utils::display::hex_dump(data, var.offset);
                    }

                }
                MemoryCommands::Write(var) => {

                    let data: Vec<u8> = match &var.data {
                      Some(d) => d.clone(),
                      None => {
                        // Read from input file
                        let input_file = match &var.input {
                          Some(f) => f,
                          None => bail!("No data provided to write, please provide data via --data or --input"),
                        };
                        std::fs::read(input_file)?
                      }
                    };

                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);
                    let device = resolve_memory_ref(&memories, &var.mem)?.clone();
                    let mem_id = device.memory_id;

                    let raw_access_memory = match cf.memory.open_memory::<RawMemory>(device).await {
                      Some(Ok(m)) => m,
                      Some(Err(e)) => bail!("Could not access memory ID={} as raw memory: {}", mem_id, e),
                      None => bail!("Memory ID={} not found", mem_id),
                    };

                    let progress_bar = utils::display::get_progressbar(data.len(), None);
                    let pb = progress_bar.clone();
                    let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
                      pb.set_position(bytes_written as u64);
                    };

                    raw_access_memory.write_with_progress(var.offset, &data, progress_callback).await?;

                    utils::display::finish_progress(&progress_bar, format!("Wrote {} bytes to memory ID={} at offset 0x{:x}", data.len(), mem_id, var.offset));

                }
                MemoryCommands::Verify(var) => {
                    let expected: Vec<u8> = match &var.data {
                      Some(d) => d.clone(),
                      None => {
                        let input_file = match &var.input {
                          Some(f) => f,
                          None => bail!("No data provided to verify against, please provide data via --data or --input"),
                        };
                        std::fs::read(input_file)?
                      }
                    };

                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);
                    let device = resolve_memory_ref(&memories, &var.mem)?.clone();

                    modules::memory::verify(&cf, device, var.offset, &expected).await?;
                }
                MemoryCommands::Display(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);

                    let device = match &var.mem {
                      Some(reference) => resolve_memory_ref(&memories, reference)?.clone(),
                      None => {
                        require_arg(non_interactive, "<mem>")?;
                        let options: Vec<String> = memories.iter().map(|mem| {
                          format!("[{}] {:?} size={}k (0x{:x}/{})", mem.memory_id, mem.memory_type, mem.size / 1024, mem.size, mem.size)
                        }).collect();

                        let selected_option = Select::new("Select a memory:", options)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No memory selected"))?;

                        let selected_id = selected_option
                          .split(']')
                          .next()
                          .and_then(|s| s.trim_start_matches('[').parse::<u8>().ok())
                          .ok_or_else(|| anyhow::anyhow!("Failed to parse memory ID"))?;

                        memories.iter()
                          .find(|m| m.memory_id == selected_id)
                          .copied()
                          .ok_or_else(|| anyhow::anyhow!("No memory with ID {}", selected_id))?
                          .clone()
                      }
                    };

                    modules::memory::display(&cf, device).await?;

                  }
                MemoryCommands::Erase(var) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);

                    let device = match &var.mem {
                      Some(reference) => resolve_memory_ref(&memories, reference)?.clone(),
                      None => {
                        require_arg(non_interactive, "<mem>")?;
                        let options: Vec<String> = memories.iter().map(|mem| {
                          format!("[{}] {:?} size={}k (0x{:x}/{})", mem.memory_id, mem.memory_type, mem.size / 1024, mem.size, mem.size)
                        }).collect();

                        let selected_option = Select::new("Select a memory:", options)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No memory selected"))?;

                        let selected_id = selected_option
                          .split(']')
                          .next()
                          .and_then(|s| s.trim_start_matches('[').parse::<u8>().ok())
                          .ok_or_else(|| anyhow::anyhow!("Failed to parse memory ID"))?;

                        memories.iter()
                          .find(|m| m.memory_id == selected_id)
                          .copied()
                          .ok_or_else(|| anyhow::anyhow!("No memory with ID {}", selected_id))?
                          .clone()
                      }
                    };

                    modules::memory::erase(&cf, device).await?;

                  }
            }
        }
        Commands::Platform { command } => {
            match command {
                PlatformCommands::Info => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    let info = modules::platform::info(cf).await?;

                    if csv {
                        println!("field,value");
                        for (field, value) in modules::platform::PlatformInfo::CSV_HEADER.iter().zip(info.csv_fields()) {
                            utils::display::csv_row(&[field, value]);
                        }
                    } else {
                        println!("Platform\t: {}", info.platform);
                        println!("Firmware\t: {}", info.firmware);
                        println!("CRTP protocol\t: {}", info.protocol);
                    }

                }
                PlatformCommands::Reboot => {
                    modules::bootloader::reboot(&link_context, uri.as_str()).await?;
                },
                PlatformCommands::PowerOff => {
                    crazyflie_lib::Crazyflie::power_off_all(&link_context, uri.as_str()).await?;
                },
                PlatformCommands::Sleep => {
                    crazyflie_lib::Crazyflie::power_off_stm32_domain(&link_context, uri.as_str()).await?;
                },
                PlatformCommands::Wakeup => {
                    crazyflie_lib::Crazyflie::power_on_stm32_domain(&link_context, uri.as_str()).await?;
                }
                PlatformCommands::Dfu => {
                    if !modules::bootloader::stm32_dfu(&link_context, uri.as_str()).await? {
                        bail!(
                            "the STM32 did not show up on USB in DFU mode. The Crazyflie has to be connected \
                             over USB, and its nRF51 firmware has to support the command. If the STM32 did \
                             restart, `cfcli platform reboot` brings it back to its firmware."
                        );
                    }
                    println!("STM32 is in USB DFU mode (0483:df11)");
                    println!("When done, restart it into its firmware with: cfcli platform reboot");
                }
            }
            
        }
        Commands::Test { command } => {
            match command {
                TestCommands::Stability(params) => {
                    modules::test::stability(&link_context, uri.as_str(), params.iterations).await?;
                }
                TestCommands::Reboot(params) => {
                    modules::test::reboot(&link_context, uri.as_str(), toc_cache, params.iterations).await?;
                }
                TestCommands::LinkPerf(params) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;
                    let test = match params.test {
                        LinkPerfTest::All => modules::test::LinkPerfTest::All,
                        LinkPerfTest::Ping => modules::test::LinkPerfTest::Ping,
                        LinkPerfTest::Uplink => modules::test::LinkPerfTest::Uplink,
                        LinkPerfTest::Downlink => modules::test::LinkPerfTest::Downlink,
                        LinkPerfTest::Echo => modules::test::LinkPerfTest::Echo,
                    };
                    modules::test::link_perf(cf, test, params.packets, params.pings, csv).await?;
                }
                TestCommands::MemPerf(params) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;
                    modules::test::mem_perf(cf, params.length, csv).await?;
                }
            }
        },
        Commands::Loco { command } => {
            // Displaying a file needs no connection.
            if let LocoCommands::Config { command: LocoConfigCommands::Display(params) } = &command {
                if let Some(file_path) = &params.input {
                    modules::lps::display_file(file_path)?;
                    return Ok(());
                }
            }

            match command {
                LocoCommands::Display => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;
                    modules::lps::display(&cf, csv, non_interactive).await?;
                }
                LocoCommands::Config { command } => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    match command {
                        LocoConfigCommands::Display(_) => {
                            modules::lps::display(&cf, csv, non_interactive).await?;
                        }
                        LocoConfigCommands::Read(params) => {
                            modules::lps::read(&cf, params.output.as_deref(), non_interactive).await?;
                        }
                        LocoConfigCommands::Write(params) => {
                            modules::lps::write(
                                &cf,
                                params.input.as_deref(),
                                !params.no_verify,
                                std::time::Duration::from_secs(params.verify_timeout),
                                non_interactive,
                            ).await?;
                        }
                    }
                }
            }
        }
        Commands::Hlc { command } => {
            // Handle trajectory display with file separately (no connection needed)
            if let HlCommands::Trajectory { command: TrajectoryCommands::Display(params) } = &command {
                if let Some(file_path) = &params.file {
                    modules::trajectory::display_file(file_path)?;
                    return Ok(());
                }
            }

            let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

            match command {
                HlCommands::Arm => {
                    println!("Arming Crazyflie...");
                    cf.supervisor.send_arming_request(true).await?;
                    println!("Crazyflie armed!");
                }
                HlCommands::Disarm => {
                    println!("Disarming Crazyflie...");
                    cf.supervisor.send_arming_request(false).await?;
                    println!("Crazyflie disarmed!");
                }
                HlCommands::Takeoff(params) => {
                    let yaw_rad = params.yaw.map(|y| y.to_radians());
                    println!("Taking off to {:.2}m over {:.1}s...", params.height, params.duration);
                    cf.high_level_commander.take_off(params.height, yaw_rad, params.duration, None).await?;
                    println!("Takeoff command sent!");
                }
                HlCommands::Land(params) => {
                    let yaw_rad = params.yaw.map(|y| y.to_radians());
                    println!("Landing to {:.2}m over {:.1}s...", params.height, params.duration);
                    cf.high_level_commander.land(params.height, yaw_rad, params.duration, None).await?;
                    println!("Land command sent!");
                }
                HlCommands::Goto(params) => {
                    let pos = &params.position;
                    let yaw = params.yaw.unwrap_or(0.0);
                    let yaw_rad = yaw.to_radians();
                    println!(
                        "Going to ({:.2}, {:.2}, {:.2}) yaw={:.1}° over {:.1}s (relative={})...",
                        pos.x, pos.y, pos.z, yaw, params.duration, params.relative
                    );
                    cf.high_level_commander.go_to(
                        pos.x, pos.y, pos.z, yaw_rad,
                        params.duration, params.relative, false, None
                    ).await?;
                    println!("Go-to command sent!");
                }
                HlCommands::Stop => {
                    println!("Stopping high-level commander...");
                    cf.high_level_commander.stop(None).await?;
                    println!("Stop command sent!");
                }
                HlCommands::Trajectory { command: traj_cmd } => {
                    match traj_cmd {
                        TrajectoryCommands::Upload(params) => {
                            modules::trajectory::upload(&cf, &params.input, params.trajectory_id, params.offset).await?;
                        }
                        TrajectoryCommands::Run(params) => {
                            modules::trajectory::run(
                                &cf,
                                params.trajectory_id,
                                params.time_scale,
                                params.relative_position,
                                params.relative_yaw,
                                params.reversed,
                            ).await?;
                        }
                        TrajectoryCommands::Display(params) => {
                            // File case handled above, this is memory display
                            if params.file.is_none() {
                                modules::trajectory::display_memory(&cf).await?;
                            }
                        }
                    }
                }
            }

        }
        Commands::Cr { command } => {
            match command {
                CrCommands::List => {
                    modules::crazyradio::list()?;
                }
                CrCommands::Sniff(params) => {
                    let address = decode_address(&params.address)?;
                    modules::crazyradio::sniff(params.radio, params.channel, params.datarate, &address).await?;
                }
                CrCommands::Broadcast(params) => {
                    let address = decode_address(&params.address)?;
                    let data: Vec<u8> = match &params.data {
                        Some(d) => d.clone(),
                        None => {
                            let input_file = match &params.input {
                                Some(f) => f,
                                None => bail!("No data provided, please provide data via --data or --input"),
                            };
                            std::fs::read(input_file)?
                        }
                    };
                    modules::crazyradio::broadcast(params.radio, params.channel, params.datarate, &address, &data).await?;
                }
            }
        }
        Commands::Debug { command } => {
            match command {
                DebugCommands::Assert(params) => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;
                    modules::debug::assert_dump(
                        cf,
                        std::time::Duration::from_millis(params.wait_timeout_ms),
                    ).await?;
                }
            }
        }
        Commands::Lh { command } => {
            match command {
                LighthouseCommands::Config { command } => {
                    if let LighthouseConfigCommands::Display(params) = &command {
                        if let Some(file_path) = &params.input {
                            modules::lighthouse::display_file(file_path)?;
                            return Ok(());
                        }
                    }

                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;

                    match command {
                        LighthouseConfigCommands::Display(_) => {
                            modules::lighthouse::display(&cf, csv, non_interactive).await?;
                        }
                        LighthouseConfigCommands::Write(params) => {
                            modules::lighthouse::write(&cf, params.input.as_deref(), non_interactive).await?;
                        }
                        LighthouseConfigCommands::Read(params) => {
                            modules::lighthouse::read(&cf, params.output.as_deref(), non_interactive).await?;
                        }
                    }
                }
            }
        }
        Commands::Swarm { command: SwarmCommands::Config { command: SwarmConfigCommands::Add(params) } }
            if params.from_usb =>
        {
            // Check the swarm before connecting to anything.
            let id = modules::swarm::swarm_to_change(&config, params.swarm.as_deref())?;
            // One Crazyflie at a time, so --name can't end up on the wrong one.
            let found = scan_usb(&link_context).await?;
            if found.is_empty() {
                bail!(CliError::Connection("no USB Crazyflies found".to_string()));
            }
            if found.len() != 1 {
                bail!(
                    "Expected exactly one Crazyflie on USB, found {} ({}). Connect only the one to add.",
                    found.len(),
                    found.join(", ")
                );
            }
            let usb_uri = &found[0];
            println!("Found Crazyflie on USB: {}", usb_uri);
            let radio_uri = radio_uri_from_usb(&mut connected_cf, &link_context, usb_uri, toc_cache, args.debug, preserve_console).await?;
            modules::swarm::add_uris(&id, params, vec![radio_uri], non_interactive)?;
        }
        Commands::Swarm { command } => {
            modules::swarm::run(&mut config, command, &link_context, toc_cache, non_interactive, csv).await?;
        }
        Commands::Deck { command } => {
            match command {
                DeckCommands::List => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;
                    modules::deck::list(&cf, csv).await?;
                }
            }
        }
        Commands::Bootload { command } => {
            match command {
                BootloadCommands::Info(params) => {
                    modules::bootloader::print_bootloader_info(&link_context, params.cold, uri.as_str()).await?;
                }
                BootloadCommands::Releases => {
                    utils::firmware::print_releases().await?;
                }
                BootloadCommands::Targets => {
                    let targets = bootloader::get_hardcoded_list_of_targets();
                    println!("Hardcoded targets:");
                    for target in targets {
                      println!("- {}", target);
                    }
                }
                BootloadCommands::Flash(params) => {
                  let source = &params.source;
                  let release = utils::flash_source::release(&source.release, non_interactive).await?;
                  let bin_with_selections = utils::flash_source::bins(&source.bin, &source.targets, non_interactive)?;

                  // The platform is normally read from the running firmware, but
                  // that isn't always possible: in cold-boot/recovery mode there is
                  // no firmware to ask, and a Crazyflie old enough to need a
                  // softdevice upgrade speaks a CRTP version we can no longer
                  // connect with. `--platform` covers both.
                  let platform = if params.cold || params.platform.is_some() {
                    match &params.platform {
                      Some(p) => utils::flash_source::platform_name(p)?.to_string(),
                      None => {
                        require_arg(non_interactive, "--platform")?;
                        // Only reachable in cold-boot mode; otherwise we would
                        // have asked the firmware instead of coming here.
                        Select::new("Select the platform:", utils::flash_source::platform_names())
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No platform selected"))?
                          .to_string()
                      }
                    }
                  } else {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache.clone(), args.debug).await?;
                    let platform = cf.platform.device_type_name().await?;
                    save_and_disconnect(connected_cf.as_ref().unwrap(), preserve_console).await;
                    connected_cf.take();
                    platform
                  };

                  // First create a list of firmwares and targets before starting the bootloading
                  let mut upgrade = utils::firmware::FirmwareUpgrade::new(&platform, &release, &source.zip, &bin_with_selections).await?;
                  let selected_target_and_types = utils::flash_source::targets(&upgrade, &source.targets, non_interactive)?;

                  upgrade.filter_targets(&selected_target_and_types);

                  if upgrade.get_target_and_types().is_empty() {
                    println!("No valid targets to flash, exiting!");
                  } else {
                    modules::bootloader::flash(
                      &link_context,
                      uri.as_str(),
                      toc_cache,
                      upgrade,
                      params.cold,
                    ).await?;
                  }
                }
            }
        }
    }
    Ok(())
    };

    let result: Result<()> = if let Some(ms) = timeout_ms {
        let deadline = std::time::Duration::from_millis(ms);
        match tokio::time::timeout(deadline, body).await {
            Ok(r) => r,
            Err(_) => {
                if is_streaming_command(&args.command) {
                    Ok(())
                } else {
                    Err(CliError::Timeout(format!("command did not complete within {} ms", ms)).into())
                }
            }
        }
    } else {
        body.await
    };

    // Save console and disconnect any remaining connection
    if let Some(ref cf) = connected_cf {
        save_and_disconnect(cf, preserve_console).await;
    }

    result
}
