use crate::utils::deckctrl::DeckConfig;
use clap::{ArgGroup, Args, Parser, Subcommand};
use clap_num::maybe_hex;
use crazyflie_lib::subsystems::memory::{EEPROMConfigMemory, MemoryType, RadioSpeed, RawMemory};
use crazyflie_lib::TocCache;
use probe_rs::probe::list::Lister;
use probe_rs::{
    flashing::{DownloadOptions},
    Permissions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process;
use std::sync::Arc;
use inquire::{Select, MultiSelect};
use indicatif::{ProgressBar, ProgressStyle};
use crazyflie_lib::Value;

pub mod modules {
    pub mod log;
    pub mod param;
    pub mod memory;
    pub mod bootloader;
    pub mod console;
    pub mod test;
}

pub mod utils {
    pub mod deckctrl;
    pub mod display;
    pub mod firmware;
}

/// Custom parser: "a=1,b=2" → { "a" => "1", "b" => "2" }
/// Supports hex values with 0x prefix: "a=0x10,b=2" → { "a" => "16", "b" => "2" }
fn parse_key_val_pairs(s: &str) -> Result<HashMap<String, String>, String> {
  let mut map = HashMap::new();

  for pair in s.split(',') {
    let mut iter = pair.splitn(2, '=');
    let key = iter.next().ok_or("Missing key")?.trim().to_string();
    let value_str = iter.next().ok_or("Missing value")?.trim();

    if key.is_empty() {
      return Err("Empty key found".into());
    }

    // Parse hex if it starts with 0x, otherwise keep as string
    let value = if let Some(hex_str) = value_str.strip_prefix("0x").or_else(|| value_str.strip_prefix("0X")) {
      // Try to parse as hex, but keep original string if parsing fails
      match u64::from_str_radix(hex_str, 16) {
        Ok(num) => num.to_string(),
        Err(_) => value_str.to_string(),
      }
    } else {
      value_str.to_string()
    };

    map.insert(key, value);
  }

  Ok(map)
}

/// Custom parser: "a=1,b=2" → { "a" => Some("1"), "b" => Some("2") }
/// Values without '=' are stored as None: "a,b=2" → { "a" => None, "b" => Some("2") }
/// Supports hex values with 0x prefix: "a=0x10,b=2" → { "a" => Some("16"), "b" => Some("2") }
fn parse_key_opt_val_pairs(s: &str) -> Result<HashMap<String, Option<String>>, String> {
  let mut map = HashMap::new();

  for pair in s.split(',') {
    let mut iter = pair.splitn(2, '=');
    let key = iter.next().ok_or("Missing key")?.trim().to_string();
    
    if key.is_empty() {
      return Err("Empty key found".into());
    }

    let value = if let Some(value_str) = iter.next() {
      let value_str = value_str.trim();
      // Parse hex if it starts with 0x, otherwise keep as string
      let parsed = if let Some(hex_str) = value_str.strip_prefix("0x").or_else(|| value_str.strip_prefix("0X")) {
        // Try to parse as hex, but keep original string if parsing fails
        match u64::from_str_radix(hex_str, 16) {
          Ok(num) => num.to_string(),
          Err(_) => value_str.to_string(),
        }
      } else {
        value_str.to_string()
      };
      Some(parsed)
    } else {
      None
    };

    map.insert(key, value);
  }

  Ok(map)
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CliArgs {
    /// Do not use TOC cache
    #[clap(short, long, action)]
    no_toc_cache: bool,

    #[clap(subcommand)]
    command: Commands,

    /// Enable debug mode
    #[clap(short, long, action)]
    debug: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Access to the log subsystem
    Log {
        #[clap(subcommand)]
        command: LogCommands,
    },

    /// Access to the parameter subsystem
    Param {
        #[clap(subcommand)]
        command: ParamCommands,
    },

    /// Access to the memory subsystem
    Mem {
        #[clap(subcommand)]
        command: MemoryCommands,
    },

    /// Configure the Crazyflie (radio settings, etc)
    Config {
        #[clap(subcommand)]
        command: ConfigCommands,
    },

    /// Various supporting utilities for the Crazyflie and its ecosystem
    Util {
        #[clap(subcommand)]
        command: UtilCommands,
    },

    Bootload {
        #[clap(subcommand)]
        command: BootloadCommands,
    },

    /// Run tests with the Crazyflie
    Test {
        #[clap(subcommand)]
        command: TestCommands,
    },

    /// Access platform functionality
    Platform {
        #[clap(subcommand)]
        command: PlatformCommands,
    },

    /// List the Crazyflies found while scanning (on the selected address)
    Scan(ScanOptions),

    /// Scan for Crazyflies and select which one to save for later interactions
    Select(ScanOptions),

    /// Print the console text from a Crazyflie
    Console {
      /// Output raw console data without processing
      #[clap(long)]
      no_format: bool,
    },
}

#[derive(Debug, Args)]
struct ScanOptions {
    /// Radio address to scan on (5 byte hex, e.g. E7E7E7E7E7)
    #[clap(value_parser, default_value = "E7E7E7E7E7")]
    address: String,
}

#[derive(Debug, Subcommand)]
enum ConfigCommands {
    /// Set an EEPROM configuration value in the Crazylfie
    Set(ConfigNameAndValue),
    /// Display the current configuration
    Display,
}

#[derive(Debug, Args)]
struct ConfigNameAndValue {
    /// Comma separated list of key value pairs:
    ///   channel: Radio channel (0-125)
    ///   address: Radio address (5 byte hex, e.g. E7E7E7E7E7)
    ///   speed  : Radio speed (0=250Kbps, 1=1Mbps, 2=2Mbps)
    ///   pitch_trim: Pitch trim (float between -20.0 and 20.0)
    ///   roll_trim : Roll trim (float between -20.0 and 20.0)
    /// Example: channel=10,address=E7E7E7E7E7,speed=2
    #[clap(value_parser, value_parser = parse_key_val_pairs, verbatim_doc_comment)]
    settings: HashMap<String, String>
}

#[derive(Debug, Subcommand)]
enum LogCommands {
    /// List all available variables
    List,
    /// Start logging and print variable values
    Print(VariablesAndPeriod),
}

#[derive(Debug, Subcommand)]
enum ParamCommands {
    /// List all available variables
    List,
    /// Read the value of a parameter
    Get(VariableName),
    /// Set the value of a parameter
    Set(VariableNameAndValue),
}

#[derive(Debug, Subcommand)]
enum UtilCommands {
    /// Utilities for the deck controller
    DeckCtrl {
        #[clap(subcommand)]
        command: DeckControlCommands,
    },
}

#[derive(Debug, Subcommand)]
enum TestCommands {
    /// Stability testing
    Stability (StabilityTestParameters),
}

#[derive(Debug, Args)]
struct StabilityTestParameters {
    /// Number of iterations to run each test
    #[clap(value_parser, default_value_t = 10)]
    iterations: u32,
}

#[derive(Debug, Subcommand)]
enum DeckControlCommands {
    /// Generate the configuration binary for the top page
    Bingen(DeckBingenParameters),
    /// Flash the configuration binary to the deck
    Binflash(DeckBinflashParameters),
}

#[derive(Debug, Subcommand)]
enum BootloadCommands {
  /// Print bootloader information
  Info(InfoParameters),
  /// List available releases
  Releases,
  /// List of hardcoded targets
  Targets,
  /// Flash firmware to the device
  Flash(FlashParameters),
}

#[derive(Debug, Args)]
struct InfoParameters {
  /// Use coldboot (i.e rescue mode) to flash the device
  #[clap(long, default_value_t = false)]
  cold: bool,
}

#[derive(Debug, Args)]
#[command(
  // group(
  //   ArgGroup::new("source_type")
  //     .args(&["release", "zip"])
  //     .required(false)
  //     .multiple(false)
  // ),
  group(
    ArgGroup::new("firmware_source")
      .args(&["release", "zip", "bin"])
      .required(true)
      .multiple(true)
  )
)]
struct FlashParameters {
  /// Release name, interactive selection if left blank (cannot be combined with zip)
  #[clap(long)]
  release: Option<Option<String>>,
  /// Release ZIP file path (cannot be combined with release)
  #[clap(long)]
  zip: Option<String>,
  /// Comma-separated list of key=value pairs for targets and binary files.
  /// Note that these will override any files in release or zip.
  /// 
  /// Example: stm32-fw=cf2_stm.bin,nrf51-fw=cf2_nrf.bin
  #[clap(long, value_parser = parse_key_opt_val_pairs, verbatim_doc_comment)]
  bin: Option<HashMap<String, Option<String>>>,
  /// Comma-separated list of targets to flash, interactive selection if
  /// left blank. By default all targets found in the release/zip/bin will
  /// be flashed.
  /// 
  /// Example: stm32-fw,nrf51-fw
  #[clap(long, verbatim_doc_comment)]
  targets: Option<Option<String>>,
  /// Do not verify flashed data
  #[clap(long, default_value_t = false)]
  no_verify: bool,
  /// Use coldboot (i.e rescue mode) to flash the device
  #[clap(long, default_value_t = false)]
  cold: bool,
}

#[derive(Debug, Args)]
struct DeckBinflashParameters {
    /// Input file (in yaml format) containing the full configuration
    #[clap(value_parser)]
    input: String,
    /// Probe index (defaults to selection if more than one debugger is connected)
    #[clap(value_parser)]
    probe_idx: Option<usize>,
}

#[derive(Debug, Args)]
struct DeckBingenParameters {
    /// Input file (in yaml format) containing the full configuration
    #[clap(value_parser)]
    input: String,
    /// File to save the read raw binary data into
    #[clap(long, short = 'o')]
    output: Option<String>,
}

#[derive(Debug, Subcommand)]
enum MemoryCommands {
    /// List all available variables
    List,
    /// Read the value of a parameter
    Read(ReadMemoryParameters),
    /// Write a list of values to memory
    Write(WriteMemoryParameters),
    /// Display memory contents in a human-readable format
    Display(SelectMemoryParameters),
    /// Erase a memory
    Erase(SelectMemoryParameters)
}

#[derive(Debug, Subcommand)]
enum PlatformCommands {
    /// Show information about the connected platform
    Info,
    /// Reboot firmware (will NOT power cycle decks, or?)
    Reboot,
    /// Power off the platform
    PowerOff,
    /// Put the platform to sleep
    Sleep,
    /// Wake up the platform
    Wakeup,
}

#[derive(Debug, Args)]
struct SelectMemoryParameters {
    /// ID of memory to read
    #[clap(value_parser, default_value = None)]
    id: Option<usize>
}

#[derive(Debug, Args)]
struct ReadMemoryParameters {
    /// ID of memory to read
    #[clap(value_parser)]
    id: usize,
    /// Offset in bytes to start reading from
    #[clap(value_parser, value_parser=maybe_hex::<usize>)]
    offset: usize,
    /// Length in bytes to read
    #[clap(value_parser, value_parser=maybe_hex::<usize>)]
    length: usize,
    /// File to save the read raw binary data into
    #[clap(long, short = 'o')]
    output: Option<String>,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("source")
        .required(true)
        .multiple(false)
        .args(&["data", "input"])
))]
struct WriteMemoryParameters {
    /// ID of memory to read
    #[clap(value_parser)]
    id: usize,
    /// Offset in bytes to start reading from
    #[clap(value_parser, value_parser=maybe_hex::<usize>)]
    offset: usize,
    /// Data to write (comma-separated list of bytes)
    #[clap(long, short = 'd', value_delimiter = ',', value_parser=maybe_hex::<u8>)]
    data: Option<Vec<u8>>,
    /// File to read raw binary data from
    #[clap(long, short = 'i')]
    input: Option<String>,
}

#[derive(Debug, Args)]
struct VariableName {
    /// Comma-separated list of parameter names (defaults to list for selection)
    /// Example: loco.mode,kalman.initialX
    #[clap(value_parser, verbatim_doc_comment)]
    names: Option<String>,
}

#[derive(Debug, Args)]
struct VariableNameAndValue {
    /// Comma separated list of parameter value pairs (defaults to list of selection)
    /// Example: usd.logging=1,loco.mode=2
    #[clap(value_parser, value_parser = parse_key_val_pairs, verbatim_doc_comment)]
    params: Option<HashMap<String, String>>
}

#[derive(Debug, Args)]
struct VariablesAndPeriod {
    /// Comma-separated list of variable names (defaults to list for selection)
    /// Example: stabilizer.roll,stabilizer.pitch
    #[clap(value_parser, verbatim_doc_comment)]
    names: Option<String>,
    /// The period in milliseconds to log at
    #[clap(value_parser, default_value_t = 100)]
    period: u16,
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
}

impl Default for Config {
    fn default() -> Self {
        println!("No configuration found, loading default values");
        Config {
            uri: "".to_string(),
            toc_cache: HashMap::new(),
        }
    }
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
    fn get_toc(&self, crc32: u32) -> Option<String> {
        match self.no_toc_cache {
            true => return None,
            false => self.config.lock().unwrap().toc_cache.get(&crc32.to_string()).cloned(),
        } 
    }
    
    fn store_toc(&self, crc32: u32, toc: &str) {
        match self.no_toc_cache {
            true => return,
            false => {
              let mut config = self.config.lock().unwrap();
              config.toc_cache.insert(crc32.to_string(), toc.to_string());
              confy::store("cf-cli", None, config.clone()).unwrap_or_else(|err| {
                  println!("Could not save configuration: {:?}", err);
              });              
            },
        }
    }
}

async fn connect_with_spinner(link_context: &crazyflie_link::LinkContext, uri: &str, toc_cache: ConfigTocCache, measure_connect_time: bool) -> Result<crazyflie_lib::Crazyflie, Box<dyn std::error::Error>> {
  let spinner = ProgressBar::new_spinner();
  spinner.set_style(
    ProgressStyle::default_spinner()
      .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
      .template("{spinner:.green} {msg}")
      .unwrap()
  );
  spinner.set_message(format!("Connecting to {}...", uri));
  spinner.enable_steady_tick(std::time::Duration::from_millis(100));

  let cf = if measure_connect_time {
    let start = std::time::Instant::now();
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache).await?;
    let duration = start.elapsed();
    spinner.println(format!("Connection time: {:.2?}", duration));
    cf
  } else {
    crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache).await?
  };

  spinner.finish_with_message(format!("Connected to {}", uri));
  Ok(cf)
}

pub fn decode_address(address: &str) -> Result<[u8; 5], Box<dyn std::error::Error>> {
    match u64::from_str_radix(&address.replace("0x", ""), 16) {
        Ok(a) if a <= 0xFFFFFFFFFF => Ok(a.to_be_bytes()[3..]
            .try_into()
            .expect("Could not convert u64 to [u8; 5]")),
        Ok(_) => {
            Err("Invalid address, please provide a valid 5 byte hexadecimal address".into())
        }
        Err(_) => {
            Err("Invalid address, please provide a valid 5 byte hexadecimal address".into())
        }
    }
}

// Example scans for Crazyflies, connect the first one and print the log and param variables TOC.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = CliArgs::parse();

    let mut config: Config = confy::load("cf-cli", None).unwrap_or_else(|err| {
        println!("Could not load config file: {:?}", err);
        Config::default()
    });

    let toc_cache = ConfigTocCache::new(config.clone(), args.no_toc_cache);

    let link_context = crazyflie_link::LinkContext::new();

    match &args.command {
        Commands::Scan(scan_options) => {
            // Scan for Crazyflies on the default address
            let address = decode_address(&scan_options.address)?;
            let found = link_context.scan(address).await?;

            for uri in found {
                println!("> {}", uri);
            }
        }
        Commands::Select(scan_options) => {
            // Scan for Crazyflies on the default address
            let address = decode_address(&scan_options.address)?;
            let found = link_context.scan(address).await?;

            if found.is_empty() {
                println!("No Crazyflies found");
                return Ok(());
            }

            let selected_uri = Select::new("Select a link:", found.clone())
                .prompt()
                .ok();

            let selected_uri = match selected_uri {
                Some(uri) => uri,
                None => {
                    process::exit(1);
                }
            };

            config.uri = selected_uri.clone();

            confy::store("cf-cli", None, config).unwrap_or_else(|err| {
                println!("Could not save configuration: {:?}", err);
            });

        }
        Commands::Console { no_format } => {
            let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

            modules::console::print(&cf, *no_format).await?;

            cf.disconnect().await;
        }
        Commands::Log { command } => {
            match command {
                LogCommands::List => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    modules::log::list(&cf).await?;

                    cf.disconnect().await;
                }
                LogCommands::Print(var) => {

                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    let names = match &var.names {
                      Some(n) => n.clone(),
                      None => {
                        let available_vars = cf.log.names();
                        let selected_vars = MultiSelect::new("Select variables to log:", available_vars)
                          .prompt()
                          .unwrap_or_else(|_| {
                          println!("No variables selected");
                          process::exit(1);
                          });
                        selected_vars.join(",")
                      }
                    };


                    modules::log::print(&cf, names.as_str(), var.period as u64).await?;

                    cf.disconnect().await;
                }
            }
        }
        Commands::Param { command } => {
            match command {
                ParamCommands::List => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::param::list(&cf).await?;
                }
                ParamCommands::Get(var) => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let names = match &var.names {
                      Some(n) => n.clone(),
                      None => {
                        let available_vars = cf.param.names();
                        let selected_vars = MultiSelect::new("Select parameters to show:", available_vars)
                          .prompt()
                          .unwrap_or_else(|_| {
                          println!("No parameters selected");
                          process::exit(1);
                          });
                        selected_vars.join(",")
                      }
                    };                    

                    modules::param::get(&cf, &names).await?;
                }
                ParamCommands::Set(params) => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let param_list = match &params.params {
                      Some(p) => p.clone(),
                      None => {
                        let available_vars = cf.param.names();
                        let available_vars: Vec<String> = available_vars
                          .into_iter()
                          .filter(|name| cf.param.is_writable(name).unwrap_or(false))
                          .collect();
                        let selected_vars = MultiSelect::new("Select parameters to set:", available_vars)
                          .prompt()
                          .unwrap_or_else(|_| {
                          println!("No parameters selected");
                          process::exit(1);
                          });

                        let mut param_map = HashMap::new();
                        for name in selected_vars {
                          let param: Value = cf.param.get(&name).await?;
                          let value: String = inquire::Text::new(&format!("[{}] {:?}:", name, param))
                            .prompt()
                            .unwrap_or_else(|_| {
                              println!("No value entered for parameter '{}'", name);
                              process::exit(1);
                            });
                          param_map.insert(name, value);
                        }
                        param_map
                      }
                    };

                    modules::param::set(&cf, &param_list).await?;
                }
            }
        }
        Commands::Util { command } => {
            match command {
                UtilCommands::DeckCtrl { command } => {
                    match command {
                        DeckControlCommands::Bingen(params) => {
                            let deck_config = DeckConfig::from_yaml(params.input.clone())?;
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
                            let bytes = deck_config.to_bytes();

                            let lister = Lister::new();
                            let probes = lister.list_all();

                            if probes.is_empty() {
                                println!("No probes found, cannot flash deck");
                                process::exit(1);
                            }

                            let probe_idx = match params.probe_idx {
                                Some(idx) => {
                                  if idx < probes.len() {
                                    idx
                                  } else {
                                    println!("Invalid probe index");
                                    process::exit(1);
                                  }
                                },
                                None => {
                                    if probes.len() == 1 {
                                        0 as usize
                                    } else {
                                        let options: Vec<String> = probes.iter().enumerate().map(|(i, p)| {
                                          format!("[{}] {} ({}:{}-{})", i, p.identifier, p.vendor_id, p.product_id, p.serial_number.as_deref().unwrap_or("N/A"))
                                        }).collect();

                                        let selected_option = Select::new("Select a probe:", options)
                                          .prompt()
                                          .unwrap_or_else(|_| {
                                            println!("No probe selected");
                                            process::exit(1);
                                          });

                                        // Extract the probe index from the selected option
                                        let idx = selected_option
                                          .split(']')
                                          .next()
                                          .and_then(|s| s.trim_start_matches('[').parse::<usize>().ok())
                                          .unwrap_or_else(|| {
                                            println!("Failed to parse probe index");
                                            process::exit(1);
                                          });
                                        idx
                                    }
                                }
                            };

                            if probes.is_empty() {
                                println!("No probes found, cannot flash deck");
                                process::exit(1);
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
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));

                    if memories.len() != 1 {
                      println!("No EEPROMConfig memory found or more than one ({}), exiting!", memories.len());
                      process::exit(1);
                    }

                    let mut eeprom_memory = match cf.memory.open_memory::<EEPROMConfigMemory>(memories[0].clone()).await {
                      Some(Ok(m)) => m,
                      Some(Err(e)) => {
                        println!("Could not access EEPROM memory: {}", e);
                        process::exit(1);
                      }
                      None => {
                        println!("No EEPROM memory found");
                        process::exit(1);
                      }
                    };

                    for (key, value) in &var.settings {
                      match key.as_str() {
                        "channel" => {
                          let channel: u8 = match value.parse() {
                            Ok(c) if c <= 125 => c,
                            _ => {
                              println!("Invalid channel value, must be an integer between 0 and 125");
                              process::exit(1);
                            }
                          };
                          eeprom_memory.set_radio_channel(channel)?;
                          println!("Set radio channel to {}", channel);
                        }
                        "address" => {
                          let address: [u8; 5] = match u64::from_str_radix(&value.replace("0x", ""), 16) {
                            Ok(a) if a <= 0xFFFFFFFFFF => a.to_be_bytes()[3..]
                                .try_into()
                                .expect("Could not convert u64 to [u8; 5]"),
                            _ => {
                                println!("Invalid address, must be a 5 byte hexadecimal value (e.g. E7E7E7E7E7)");
                                process::exit(1);
                            }
                          };
                          eeprom_memory.set_radio_address(address);
                          println!("Set radio address to {:02X?}", address);
                        }
                        "speed" => {
                          let speed: u8 = match value.parse() {
                            Ok(s) if s <= 2 => s,
                            _ => {
                              println!("Invalid speed value, must be 0 (250Kbps), 1 (1Mbps) or 2 (2Mbps)");
                              process::exit(1);
                            }
                          };
                          eeprom_memory.set_radio_speed(RadioSpeed::try_from(speed)?);
                          println!("Set radio speed to {}", speed);
                        }
                        "pitch_trim" => {
                          let pitch_trim: f32 = match value.parse() {
                            Ok(p) if p >= -20.0 && p <= 20.0 => p,
                            _ => {
                              println!("Invalid pitch trim value, must be a float between -20.0 and 20.0");
                              process::exit(1);
                            }
                          };
                          eeprom_memory.set_pitch_trim(pitch_trim);
                          println!("Set pitch trim to {}", pitch_trim);
                        }
                        "roll_trim" => {
                          let roll_trim: f32 = match value.parse() {
                            Ok(r) if r >= -20.0 && r <= 20.0 => r,
                            _ => {
                              println!("Invalid roll trim value, must be a float between -20.0 and 20.0");
                              process::exit(1);
                            }
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

                    cf.disconnect().await;
                }
                ConfigCommands::Display => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));

                    if memories.len() != 1 {
                      println!("No EEPROMConfig memory found or more than one ({}), exiting!", memories.len());
                      process::exit(1);
                    }

                    modules::memory::display(&cf, memories[0].clone()).await;

                    cf.disconnect().await;
                  }
            }
        }
        Commands::Mem { command } => {
            match command {
                MemoryCommands::List => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memory = cf.memory.get_memories(None);

                    println!("Memories:");
                    for mem in memory {
                      let memory_serial = mem.serial.as_ref()
                        .map(|s| format!(" (0x{})", s.iter().map(|b| format!("{:02X}", b)).collect::<String>()))
                        .unwrap_or_default();
                      println!("[{}] {:?} size={}k (0x{:x}/{}){}", mem.memory_id, mem.memory_type, mem.size / 1024, mem.size, mem.size, memory_serial);
                    }


                    cf.disconnect().await;
                }
                MemoryCommands::Read(var) => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);

                    let raw_access_memory = match cf.memory.open_memory::<RawMemory>(memories[var.id].clone()).await {
                      Some(Ok(m)) => m,
                      Some(Err(e)) => {
                        println!("Could not access memory ID={} as raw memory: {}", var.id, e);
                        process::exit(1);
                      }
                      None => {
                        println!("Memory ID={} not found", var.id);
                        process::exit(1);
                      }
                    };

                    if let Some(output_file) = &var.output {

                        let progress_bar = utils::display::get_progressbar(var.length, None);   
                        let pb = progress_bar.clone();
                        let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
                          pb.set_position(bytes_written as u64);
                        };
                        let data = raw_access_memory.read_with_progress(var.offset, var.length, progress_callback).await?;

                        progress_bar.finish_with_message(format!("Read {} bytes from memory ID={} at offset 0x{:x}", var.length, var.id, var.offset));

                      std::fs::write(output_file, &data).unwrap_or_else(|e| {
                        println!("Could not write to output file {}: {}", output_file, e);
                        process::exit(1);
                      });
                    } else {
                      let data = raw_access_memory.read(var.offset, var.length).await?;
                      utils::display::hex_dump(data, var.offset);
                    }

                    cf.disconnect().await;
                }
                MemoryCommands::Write(var) => {

                    let data: Vec<u8> = match &var.data {
                      Some(d) => d.clone(),
                      None => {
                        // Read from input file
                        let input_file = match &var.input {
                          Some(f) => f,
                          None => {
                            println!("No data provided to write, please provide data via --data or --input");
                            process::exit(1);
                          }
                        };
                        std::fs::read(input_file).unwrap_or_else(|e| {
                          println!("Could not read input file {}: {}", input_file, e);
                          process::exit(1);
                        })
                      }
                    };

                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);

                    let raw_access_memory = match cf.memory.open_memory::<RawMemory>(memories[var.id].clone()).await {
                      Some(Ok(m)) => m,
                      Some(Err(e)) => {
                        println!("Could not access memory ID={} as raw memory: {}", var.id, e);
                        process::exit(1);
                      }
                      None => {
                        println!("Memory ID={} not found", var.id);
                        process::exit(1);
                      }
                    };

                    let progress_bar = utils::display::get_progressbar(data.len(), None);   
                    let pb = progress_bar.clone();
                    let progress_callback = move |bytes_written: usize, _total_bytes: usize| {
                      pb.set_position(bytes_written as u64);
                    };

                    raw_access_memory.write_with_progress(var.offset, &data, progress_callback).await?;

                    progress_bar.finish_with_message(format!("Wrote {} bytes to memory ID={} at offset 0x{:x}", data.len(), var.id, var.offset));

                    cf.disconnect().await;
                }
                MemoryCommands::Display(var) => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);

                    let selected_id = match var.id {
                      Some(id) => id,
                      None => {
                        let options: Vec<String> = memories.iter().map(|mem| {
                          format!("[{}] {:?} size={}k (0x{:x}/{})", mem.memory_id, mem.memory_type, mem.size / 1024, mem.size, mem.size)
                        }).collect();

                        let selected_option = Select::new("Select a memory:", options)
                          .prompt()
                          .unwrap_or_else(|_| {
                            println!("No memory selected");
                            process::exit(1);
                          });

                        // Extract the memory ID from the selected option
                        let selected_id = selected_option
                          .split(']')
                          .next()
                          .and_then(|s| s.trim_start_matches('[').parse::<usize>().ok())
                          .unwrap_or_else(|| {
                            println!("Failed to parse memory ID");
                            process::exit(1);
                          });

                        selected_id
                      }
                        
                    };

                    modules::memory::display(&cf, memories[selected_id].clone()).await;

                    cf.disconnect().await;
                  }
                MemoryCommands::Erase(var) => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let memories = cf.memory.get_memories(None);

                    let selected_id = match var.id {
                      Some(id) => id,
                      None => {
                        let options: Vec<String> = memories.iter().map(|mem| {
                          format!("[{}] {:?} size={}k (0x{:x}/{})", mem.memory_id, mem.memory_type, mem.size / 1024, mem.size, mem.size)
                        }).collect();

                        let selected_option = Select::new("Select a memory:", options)
                          .prompt()
                          .unwrap_or_else(|_| {
                            println!("No memory selected");
                            process::exit(1);
                          });

                        // Extract the memory ID from the selected option
                        let selected_id = selected_option
                          .split(']')
                          .next()
                          .and_then(|s| s.trim_start_matches('[').parse::<usize>().ok())
                          .unwrap_or_else(|| {
                            println!("Failed to parse memory ID");
                            process::exit(1);
                          });

                        selected_id
                      }
                        
                    };

                    if selected_id >= memories.len() {
                      println!("Invalid memory ID selected");
                      process::exit(1);
                    }

                    modules::memory::erase(&cf, memories[selected_id].clone()).await;

                    cf.disconnect().await;
                  }
            }
        }
        Commands::Platform { command } => {
            match command {
                PlatformCommands::Info => {
                    let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache, args.debug).await?;

                    let protocol_version = cf.platform.protocol_version().await?;
                    let firmware_version = cf.platform.firmware_version().await?;
                    let device_type_name = cf.platform.device_type_name().await?;

                    println!("Platform\t: {}", device_type_name);
                    println!("Firmware\t: {}", firmware_version);
                    println!("CRTP protocol\t: {}", protocol_version);

                    cf.disconnect().await;
                }
                PlatformCommands::Reboot => {
                    modules::bootloader::reboot(&link_context, config.uri.as_str()).await?;
                },
                PlatformCommands::PowerOff => {
                    modules::bootloader::power_off(&link_context, config.uri.as_str()).await?;
                },
                PlatformCommands::Sleep => {
                    modules::bootloader::sysoff(&link_context, config.uri.as_str()).await?;
                },
                PlatformCommands::Wakeup => {
                    modules::bootloader::syson(&link_context, config.uri.as_str()).await?;
                }
            }
            
        }
        Commands::Test { command } => {
            match command {
                TestCommands::Stability(params) => {
                    modules::test::stability(&link_context, config.uri.as_str(), params.iterations).await?;
                }
            }
        },
        Commands::Bootload { command } => {
            match command {
                BootloadCommands::Info(params) => {
                    modules::bootloader::print_bootloader_info(&link_context, params.cold, config.uri.as_str()).await?;
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
                  let release = match &params.release {
                    Some(Some(r)) => {
                      let labels = utils::firmware::get_release_labels().await?;
                      if !labels.contains(r) {
                        println!("Release '{}' not found", r);
                        process::exit(1);
                      }
                      Some(r.clone())
                    },
                    Some(None) => {
                      let labels = utils::firmware::get_release_labels().await?;
                      let selected_release = Select::new("Select a firmware release to flash:", labels)

                        .prompt()
                        .unwrap_or_else(|_| {
                          println!("No release selected");
                          process::exit(1);
                        });
                      Some(selected_release)
                    }
                    None => None,
                  };

                  // This case is special since we're not setting the key on the command-line,
                  // we're actually setting the value and then we'll select they key here
                  // Note that the list of tarets is hardcoded, this is because we cannot
                  // query the Crazyflie for it, flashing new firmware might change this
                  // until we reach the deck flashing stage.
                  let bin_with_selections = {
                    let mut result = HashMap::new();
                    if let Some(bin_map) = &params.bin {
                      for (key, value_opt) in bin_map.iter() {
                        let (k,v) = match (key, value_opt) {
                          (k, Some(v)) => (k.clone(), v.clone()),
                          (k, None) => {
                            let selected_target = Select::new(
                              "Select binary to flash:",
                              bootloader::get_hardcoded_list_of_targets()
                            )
                            .prompt()
                            .unwrap_or_else(|_| {
                              println!("No binary selected");
                              process::exit(1);
                            });
                            (selected_target.to_string(), k.to_string())
                          }
                        };
                        result.insert(k, v);
                      }
                    }
                    Some(result)
                  };

                  let cf = connect_with_spinner(&link_context, config.uri.as_str(), toc_cache.clone(), args.debug).await?;
                  let platform = cf.platform.device_type_name().await?;
                  cf.disconnect().await;

                  // First create a list of firmwares and targets before starting the bootloading
                  let mut upgrade = utils::firmware::FirmwareUpgrade::new(&platform, &release, &params.zip, &bin_with_selections).await?;

                  let selected_target_and_types = match &params.targets {
                    Some(Some(t)) => t.split(',').map(|s| s.trim().to_string()).collect(),
                    Some(None) => {
                      let available_target_and_types = upgrade.get_target_and_types();

                      let selected_target_and_types = MultiSelect::new("Select targets to flash:", available_target_and_types)
                        .prompt()
                        .unwrap_or_else(|_| {
                          println!("No targets selected");
                          process::exit(1);
                        });
                      selected_target_and_types
                    }
                    None => upgrade.get_target_and_types(),
                  };

                  upgrade.filter_targets(&selected_target_and_types);

                  if upgrade.get_target_and_types().is_empty() {
                    println!("No valid targets to flash, exiting!");
                  } else {
                    modules::bootloader::flash(
                      &link_context,
                      config.uri.as_str(),
                      toc_cache,
                      upgrade,
                      params.no_verify,
                      params.cold,
                    ).await?;
                  }
                }
            }
        }
    }

    Ok(())
}
