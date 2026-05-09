use crate::modules::bootloader;
use crate::utils::deckctrl::DeckConfig;
use clap::{ArgGroup, Args, Parser, Subcommand};
use clap_num::maybe_hex;
use crazyflie_lib::subsystems::memory::{EEPROMConfigMemory, MemoryDevice, MemoryType, RadioSpeed, RawMemory};
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
use inquire::{Select, MultiSelect};
use crazyflie_lib::Value;
use anyhow::{bail, Result};

pub mod error;

pub mod modules {
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
}

pub mod utils {
    pub mod deckctrl;
    pub mod display;
    pub mod firmware;
}

use error::CliError;

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

/// Position parsed from comma-separated values: "x,y,z"
#[derive(Debug, Clone)]
struct Position {
    x: f32,
    y: f32,
    z: f32,
}

fn parse_position(s: &str) -> Result<Position, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err("Position must be x,y,z (comma-separated)".to_string());
    }

    let x = parts[0].trim().parse::<f32>()
        .map_err(|_| format!("Invalid x value: {}", parts[0]))?;
    let y = parts[1].trim().parse::<f32>()
        .map_err(|_| format!("Invalid y value: {}", parts[1]))?;
    let z = parts[2].trim().parse::<f32>()
        .map_err(|_| format!("Invalid z value: {}", parts[2]))?;

    Ok(Position { x, y, z })
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

/// A reference to a memory, parsed from the CLI.
///
/// Accepts either:
///   - a numeric memory ID (decimal or `0x`-prefixed hex), e.g. `5` or `0x05`
///   - a `MemoryType` variant name, e.g. `DeckCtrlDFU`
///   - a type with an instance suffix to disambiguate when multiple memories
///     of the same type exist, e.g. `DeckCtrlDFU:0`
#[derive(Debug, Clone)]
enum MemoryRef {
    Id(u8),
    Type(MemoryType, Option<usize>),
}

fn parse_memory_type(s: &str) -> Result<MemoryType, String> {
    match s {
        "EEPROMConfig" => Ok(MemoryType::EEPROMConfig),
        "OneWire" => Ok(MemoryType::OneWire),
        "DriverLed" => Ok(MemoryType::DriverLed),
        "Loco" => Ok(MemoryType::Loco),
        "Trajectory" => Ok(MemoryType::Trajectory),
        "Loco2" => Ok(MemoryType::Loco2),
        "Lighthouse" => Ok(MemoryType::Lighthouse),
        "MemoryTester" => Ok(MemoryType::MemoryTester),
        "MicroSD" => Ok(MemoryType::MicroSD),
        "DriverLedTiming" => Ok(MemoryType::DriverLedTiming),
        "App" => Ok(MemoryType::App),
        "DeckMemory" => Ok(MemoryType::DeckMemory),
        "DeckCtrlDFU" => Ok(MemoryType::DeckCtrlDFU),
        "DeckCtrl" => Ok(MemoryType::DeckCtrl),
        "DeckMultiranger" => Ok(MemoryType::DeckMultiranger),
        "DeckPaa3905" => Ok(MemoryType::DeckPaa3905),
        _ => Err(format!(
            "Unknown memory type '{}'. Valid types: EEPROMConfig, OneWire, DriverLed, \
             Loco, Trajectory, Loco2, Lighthouse, MemoryTester, MicroSD, DriverLedTiming, \
             App, DeckMemory, DeckCtrlDFU, DeckCtrl, DeckMultiranger, DeckPaa3905",
            s
        )),
    }
}

fn parse_memory_ref(s: &str) -> Result<MemoryRef, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u8::from_str_radix(hex, 16)
            .map(MemoryRef::Id)
            .map_err(|e| format!("Invalid hex memory ID '{}': {}", s, e));
    }
    if let Ok(id) = s.parse::<u8>() {
        return Ok(MemoryRef::Id(id));
    }

    let (type_str, instance) = match s.split_once(':') {
        Some((t, n)) => {
            let n = n.parse::<usize>().map_err(|_| {
                format!("Invalid instance index '{}': must be a non-negative integer", n)
            })?;
            (t, Some(n))
        }
        None => (s, None),
    };
    Ok(MemoryRef::Type(parse_memory_type(type_str)?, instance))
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
            let matches: Vec<&MemoryDevice> = memories
                .iter()
                .filter(|m| m.memory_type == *mt)
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

// "Exit codes:" mirrors clap's default header style (bold + underline).
// anstream strips the ANSI escapes when stdout isn't a terminal.
const HELP_EPILOG: &str = "\x1b[1m\x1b[4mExit codes:\x1b[0m
   0  success
   1  unspecified error
   2  usage / argument error (clap)
  10  connection failure (no Crazyflie found, link error, disconnected)
  20  resource not found (param/log/memory by name, release name)
  30  invalid value (range, type, malformed input)
  40  --timeout expired on a bounded command
";

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None, after_help = HELP_EPILOG)]
struct CliArgs {
    /// Do not use TOC cache
    #[clap(short, long, action)]
    no_toc_cache: bool,

    #[clap(subcommand)]
    command: Commands,

    /// Enable debug mode
    #[clap(short, long, action)]
    debug: bool,

    /// Override the URI to connect to (instead of using the config file)
    #[clap(short, long)]
    uri: Option<String>,

    /// Preserve console output across connections, printed when the 'console' command is run
    #[clap(short, long, action)]
    preserve_console: bool,

    /// Timeout in milliseconds for the command
    #[clap(long, global = true)]
    timeout: Option<u64>,

    /// Disable interactive prompts (auto-set when stdin is not a TTY)
    #[clap(long, global = true)]
    non_interactive: bool,

    /// Emit machine-readable CSV (for read commands that support it)
    #[clap(long, global = true)]
    csv: bool,
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

    /// Bootload the Crazyflie and decks
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
    Select(SelectOptions),

    /// Print the console text from a Crazyflie
    Console {
      /// Output raw console data without processing
      #[clap(long)]
      no_format: bool,

      /// Delete the preserved console history file and exit without connecting
      #[clap(long)]
      clear: bool,
    },

    /// Local CLI settings (scan addresses, timeout, etc.)
    Settings {
        #[clap(subcommand)]
        command: SettingsCommands,
    },

    /// Loco Positioning System
    Loco {
        #[clap(subcommand)]
        command: LocoCommands,
    },

    /// High-level commander operations (takeoff, land, go-to, trajectory, etc.)
    Hlc {
        #[clap(subcommand)]
        command: HlCommands,
    },

    /// Crazyradio operations (sniffer, etc.)
    Cr {
        #[clap(subcommand)]
        command: CrCommands,
    },

    /// Debugging utilities (assert info dumps, etc.)
    Debug {
        #[clap(subcommand)]
        command: DebugCommands,
    },
}

#[derive(Debug, Subcommand)]
enum DebugCommands {
    /// Trigger a firmware assert-info dump on the console
    Assert(AssertArgs),
}

#[derive(Debug, Args)]
struct AssertArgs {
    /// How long to wait for the assert line before giving up
    #[clap(long, default_value_t = 1500)]
    wait_timeout_ms: u64,
}

#[derive(Debug, Args)]
struct ScanOptions {
    /// Radio address to scan on (5 byte hex, e.g. E7E7E7E7E7). Overrides settings.
    #[clap(value_parser)]
    address: Option<String>,
}

#[derive(Debug, Args)]
struct SelectOptions {
    /// Radio address to scan on (5 byte hex, e.g. E7E7E7E7E7). Overrides settings.
    #[clap(value_parser)]
    address: Option<String>,
    /// Automatically select the URI if exactly one Crazyflie is found
    #[clap(long)]
    auto: bool,
    /// Connect to a USB-attached Crazyflie, read its radio config, and select that radio URI
    #[clap(long)]
    from_usb: bool,
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

use modules::settings;

#[derive(Debug, Subcommand)]
enum SettingsCommands {
    /// Show all current settings
    Show,
    /// Manage the connection timeout
    Timeout {
        #[clap(subcommand)]
        command: SettingsTimeoutCommands,
    },
    /// Manage scan addresses
    Address {
        #[clap(subcommand)]
        command: SettingsAddressCommands,
    },
}

#[derive(Debug, Subcommand)]
enum SettingsTimeoutCommands {
    /// Show the current timeout
    Show,
    /// Set the timeout in milliseconds
    Set {
        /// Timeout in milliseconds
        #[clap(value_parser)]
        timeout_ms: u32,
    },
    /// Reset timeout to default (1000ms)
    Clear,
}

#[derive(Debug, Subcommand)]
enum SettingsAddressCommands {
    /// List configured scan addresses
    List,
    /// Add a scan address (5 byte hex, e.g. E7E7E7E7E7)
    Add {
        /// Radio address (5 byte hex, e.g. E7E7E7E7E7)
        #[clap(value_parser)]
        address: String,
    },
    /// Remove a scan address
    Remove {
        /// Radio address to remove (5 byte hex, e.g. E7E7E7E7E7)
        #[clap(value_parser)]
        address: String,
    },
    /// Clear all addresses and reset to default (E7E7E7E7E7)
    Clear,
}

#[derive(Debug, Subcommand)]
enum CrCommands {
    /// List connected Crazyradio dongles
    List,
    /// Sniff broadcast packets on a given address
    Sniff(SniffArgs),
    /// Broadcast a message on a given address (no ack)
    Broadcast(BroadcastArgs),
}

#[derive(Debug, Args)]
struct SniffArgs {
    /// Crazyradio index (0-based)
    #[clap(short, long, default_value_t = 0)]
    radio: usize,
    /// Radio channel (0-125)
    #[clap(short, long, default_value_t = 80)]
    channel: u8,
    /// Datarate: 0=250K, 1=1M, 2=2M
    #[clap(short, long, default_value_t = 2)]
    datarate: u8,
    /// Broadcast address (5 byte hex, e.g. E7E7E7E7E7)
    #[clap(short, long, default_value = "E7E7E7E7E7")]
    address: String,
}

#[derive(Debug, Args)]
struct BroadcastArgs {
    /// Crazyradio index (0-based)
    #[clap(short, long, default_value_t = 0)]
    radio: usize,
    /// Radio channel (0-125)
    #[clap(short, long, default_value_t = 80)]
    channel: u8,
    /// Datarate: 0=250K, 1=1M, 2=2M
    #[clap(short, long, default_value_t = 2)]
    datarate: u8,
    /// Broadcast address (5 byte hex, e.g. FFE7E7E7E7)
    #[clap(short, long, default_value = "FFE7E7E7E7")]
    address: String,
    /// Data to broadcast (comma-separated list of bytes, supports hex with 0x prefix)
    #[clap(long, value_delimiter = ',', value_parser=maybe_hex::<u8>)]
    data: Option<Vec<u8>>,
    /// File to read raw binary data from
    #[clap(long, short = 'i')]
    input: Option<String>,
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
    /// Store the current value of a parameter to EEPROM
    Store(VariableName),
    /// Clear a stored parameter value from EEPROM (reverts to firmware default)
    Clear(VariableName),
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
    /// Reboot test: repeatedly reboot and check selftest and console
    Reboot (RebootTestParameters),
    /// Link performance benchmark using the CRTP link service
    /// (echo / source / sink channels on port 15)
    LinkPerf (LinkPerfTestParameters),
    /// Memory tester throughput: write/read-back the firmware MemoryTester
    MemPerf (MemPerfTestParameters),
}

#[derive(Debug, Args)]
struct StabilityTestParameters {
    /// Number of iterations to run each test
    #[clap(value_parser, default_value_t = 10)]
    iterations: u32,
}

#[derive(Debug, Args)]
struct RebootTestParameters {
    /// Number of reboot iterations to run
    #[clap(value_parser, default_value_t = 10)]
    iterations: u32,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum LinkPerfTest {
    /// Run all tests (default)
    All,
    /// Ping only (round-trip latency)
    Ping,
    /// Uplink only (sink channel)
    Uplink,
    /// Downlink only (source channel)
    Downlink,
    /// Round-trip only (echo channel with full payload)
    Echo,
}

#[derive(Debug, Args)]
struct LinkPerfTestParameters {
    /// Which test(s) to run
    #[clap(short, long, value_enum, default_value_t = LinkPerfTest::All)]
    test: LinkPerfTest,

    /// Number of packets per bandwidth test
    #[clap(long, value_parser, default_value_t = 1000)]
    packets: u64,

    /// Number of ping samples
    #[clap(long, value_parser, default_value_t = 10)]
    pings: u32,
}

#[derive(Debug, Args)]
struct MemPerfTestParameters {
    /// Length in bytes to write and read back (defaults to the full tester size, 4096)
    #[clap(long, short = 'n', default_value = "4096", value_parser = maybe_hex::<usize>)]
    length: usize,
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
  /// Optionally append @address or @page to the target to override the
  /// flash start location. Use 0x prefix for addresses, bare numbers for pages.
  ///
  /// Example: stm32-fw=cf2_stm.bin,nrf51-fw=cf2_nrf.bin
  /// Example: stm32-fw@0x08004000=custom.bin (flash at address 0x08004000)
  /// Example: stm32-fw@16=custom.bin (flash at page 16)
  #[clap(long, value_parser = parse_key_opt_val_pairs, verbatim_doc_comment)]
  bin: Option<HashMap<String, Option<String>>>,
  /// Comma-separated list of targets to flash, interactive selection if
  /// left blank. By default all targets found in the release/zip/bin will
  /// be flashed.
  /// 
  /// Example: stm32-fw,nrf51-fw
  #[clap(long, verbatim_doc_comment)]
  targets: Option<Option<String>>,
  /// Use coldboot (i.e rescue mode) to flash the device
  #[clap(long, default_value_t = false)]
  cold: bool,
  /// Platform to use when cold-booting (skips connecting to running firmware).
  /// If not specified in cold-boot mode, you will be prompted to select one.
  ///
  /// Valid values: cf21, cf21bl, bolt11, flapper, tag
  #[clap(long, verbatim_doc_comment)]
  platform: Option<String>,
}

#[derive(Debug, Args)]
struct DeckBinflashParameters {
    /// Input file (in yaml format) containing the full configuration
    #[clap(value_parser)]
    input: String,
    /// Probe index (defaults to interactive selection if more than one debugger is connected)
    #[clap(long, short = 'p')]
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

#[derive(Debug, Subcommand)]
enum TrajectoryCommands {
    /// Upload a trajectory from a YAML file
    Upload(TrajectoryUploadParameters),
    /// Run a previously uploaded trajectory
    Run(TrajectoryRunParameters),
    /// Display trajectory information (memory info or file contents)
    Display(TrajectoryDisplayParameters),
}

#[derive(Debug, Args)]
struct TrajectoryUploadParameters {
    /// Trajectory ID to assign
    #[clap(value_parser, default_value = "1")]
    trajectory_id: u8,
    /// Path to the trajectory YAML file
    #[clap(long, short = 'i')]
    input: String,
    /// Memory offset to write trajectory to
    #[clap(long, short = 's', default_value = "0", value_parser=maybe_hex::<u32>)]
    offset: u32,
}

#[derive(Debug, Args)]
struct TrajectoryRunParameters {
    /// Trajectory ID to run
    #[clap(value_parser)]
    trajectory_id: u8,
    /// Time scale factor (1.0 = normal speed, >1.0 = slower, <1.0 = faster)
    #[clap(long, short = 'f', default_value = "1.0")]
    time_scale: f32,
    /// Use relative position (shift trajectory to current position)
    #[clap(long, short = 'r')]
    relative_position: bool,
    /// Use relative yaw (align trajectory yaw to current yaw)
    #[clap(long, short = 'y')]
    relative_yaw: bool,
    /// Run trajectory in reverse
    #[clap(long)]
    reversed: bool,
}

#[derive(Debug, Args)]
struct TrajectoryDisplayParameters {
    /// Path to a trajectory YAML file to display (optional, shows memory info if omitted)
    #[clap(value_parser)]
    file: Option<String>,
}

#[derive(Debug, Subcommand)]
enum LocoCommands {
    /// Display Loco Positioning System anchor information
    Display,
}

#[derive(Debug, Subcommand)]
enum HlCommands {
    /// Arm the Crazyflie (enable motors)
    Arm,
    /// Disarm the Crazyflie (disable motors)
    Disarm,
    /// Take off to a specified height
    Takeoff(HlTakeoffParameters),
    /// Land at the current position
    Land(HlLandParameters),
    /// Go to a specified position
    Goto(HlGotoParameters),
    /// Stop all high-level commands and disable motors
    Stop,
    /// Trajectory operations
    Trajectory {
        #[clap(subcommand)]
        command: TrajectoryCommands,
    },
}

#[derive(Debug, Args)]
struct HlTakeoffParameters {
    /// Target height in meters
    #[clap(long, short = 'z', default_value = "0.5")]
    height: f32,
    /// Duration in seconds to reach the target height
    #[clap(long, short = 'd', default_value = "2.0")]
    duration: f32,
    /// Target yaw in degrees (omit to maintain current yaw)
    #[clap(long, short = 'y')]
    yaw: Option<f32>,
}

#[derive(Debug, Args)]
struct HlLandParameters {
    /// Target height in meters (typically 0.0)
    #[clap(long, short = 'z', default_value = "0.0")]
    height: f32,
    /// Duration in seconds to land
    #[clap(long, short = 'd', default_value = "2.0")]
    duration: f32,
    /// Target yaw in degrees (omit to maintain current yaw)
    #[clap(long, short = 'y')]
    yaw: Option<f32>,
}

#[derive(Debug, Args)]
struct HlGotoParameters {
    /// Target position as x,y,z (comma-separated)
    #[clap(value_parser = parse_position, allow_hyphen_values = true)]
    position: Position,
    /// Duration in seconds to reach the target position
    #[clap(long, short = 'd', default_value = "2.0")]
    duration: f32,
    /// Target yaw in degrees (default: 0)
    #[clap(long, short = 'y')]
    yaw: Option<f32>,
    /// Use relative positioning (relative to current position)
    #[clap(long, short = 'r')]
    relative: bool,
}

#[derive(Debug, Args)]
struct SelectMemoryParameters {
    /// Memory to operate on: numeric ID, type name (e.g. DeckCtrlDFU),
    /// or type with instance index (e.g. DeckCtrlDFU:0). Omit to select interactively.
    #[clap(value_parser = parse_memory_ref)]
    mem: Option<MemoryRef>,
}

#[derive(Debug, Args)]
struct ReadMemoryParameters {
    /// Memory to read from: numeric ID, type name (e.g. DeckCtrlDFU),
    /// or type with instance index (e.g. DeckCtrlDFU:0)
    #[clap(value_parser = parse_memory_ref)]
    mem: MemoryRef,
    /// Offset in bytes to start reading from
    #[clap(long, short = 's', default_value = "0", value_parser = maybe_hex::<usize>)]
    offset: usize,
    /// Length in bytes to read
    #[clap(long, short = 'n', default_value = "32", value_parser = maybe_hex::<usize>)]
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
    /// Memory to write to: numeric ID, type name (e.g. DeckCtrlDFU),
    /// or type with instance index (e.g. DeckCtrlDFU:0)
    #[clap(value_parser = parse_memory_ref)]
    mem: MemoryRef,
    /// Offset in bytes to start writing at
    #[clap(long, short = 's', value_parser = maybe_hex::<usize>)]
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
    params: Option<HashMap<String, String>>,
    /// Store the parameter(s) to EEPROM after setting
    #[clap(long)]
    store: bool,
}

#[derive(Debug, Args)]
struct VariablesAndPeriod {
    /// Comma-separated list of variable names (defaults to list for selection)
    /// Example: stabilizer.roll,stabilizer.pitch
    #[clap(value_parser, verbatim_doc_comment)]
    names: Option<String>,
    /// The period in milliseconds to log at
    #[clap(long, short = 'p', default_value_t = 100)]
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
    #[serde(default)]
    timeout_ms: Option<u32>,
    #[serde(default = "default_addresses")]
    addresses: Vec<String>,
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
        }
    }
}

impl Config {
    fn effective_timeout(&self) -> u32 {
        self.timeout_ms.unwrap_or(1000)
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
    matches!(
        cmd,
        Commands::Console { .. }
            | Commands::Log { command: LogCommands::Print(_) }
            | Commands::Cr { command: CrCommands::Sniff(_) }
    )
}

/// Connect to a Crazyflie and store the resulting handle in `holder` so the
/// centralized cleanup at the end of `run()` can disconnect it. Returns a
/// borrowed reference to the just-stored Crazyflie. The mutable borrow ends
/// when this function returns; callers can immediately use the `&Crazyflie`
/// alongside other immutable accesses to `holder`.
async fn connect_cf<'a>(
    holder: &'a mut Option<crazyflie_lib::Crazyflie>,
    link_context: &crazyflie_link::LinkContext,
    uri: &str,
    toc_cache: ConfigTocCache,
    measure_connect_time: bool,
) -> Result<&'a crazyflie_lib::Crazyflie> {
    let start = if measure_connect_time { Some(std::time::Instant::now()) } else { None };
    let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache).await
        .map_err(|e| CliError::Connection(format!("connecting to {}: {}", uri, e)))?;
    if let Some(s) = start {
        eprintln!("Connection time: {:.2?}", s.elapsed());
    }
    Ok(holder.insert(cf))
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

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {:#}", e);
            error::classify_exit_code(&e)
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<()> {
    let args = CliArgs::parse();

    let mut config: Config = confy::load("cf-cli", None).unwrap_or_else(|err| {
        println!("Could not load config file: {:?}", err);
        Config::default()
    });

    let uri = {
        let base = args.uri.clone().unwrap_or(config.uri.clone());
        if config.timeout_ms.is_some() && !base.starts_with("usb://") {
            let timeout = config.effective_timeout();
            if base.contains('?') {
                format!("{}&timeout={}", base, timeout)
            } else {
                format!("{}?timeout={}", base, timeout)
            }
        } else {
            base
        }
    };

    let toc_cache = ConfigTocCache::new(config.clone(), args.no_toc_cache);

    #[cfg(feature = "packet_capture")]
    crazyflie_link::capture::init();

    let link_context = crazyflie_link::LinkContext::new();

    let mut connected_cf: Option<crazyflie_lib::Crazyflie> = None;
    let preserve_console = args.preserve_console;
    let timeout_ms = args.timeout;
    let non_interactive = args.non_interactive || !std::io::stdin().is_terminal();
    let csv = args.csv;

    let body = async {
    match &args.command {
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
            let selected_uri = if select_options.from_usb {
                // Scan for USB-connected Crazyflies
                // USB scan only needs one address since USB devices are found regardless
                let address = decode_address("E7E7E7E7E7")?;
                let all_found = link_context.scan(address).await?;
                let found: Vec<_> = all_found.into_iter().filter(|uri| uri.starts_with("usb://")).collect();

                if found.is_empty() {
                    bail!(CliError::Connection("no USB Crazyflies found".to_string()));
                }
                if found.len() != 1 {
                    bail!("Expected exactly one Crazyflie on USB, found {}", found.len());
                }

                let usb_uri = &found[0];
                println!("Found Crazyflie on USB: {}", usb_uri);

                // Connect via USB and read EEPROM config
                let cf = connect_cf(&mut connected_cf, &link_context, usb_uri, toc_cache, args.debug).await?;

                let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));
                if memories.len() != 1 {
                    bail!("No EEPROMConfig memory found or more than one ({})", memories.len());
                }

                let eeprom = match cf.memory.open_memory::<EEPROMConfigMemory>(memories[0].clone()).await {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => bail!("Could not read EEPROM config: {}", e),
                    None => bail!("No EEPROM memory found"),
                };

                let channel = eeprom.get_radio_channel();
                let address = eeprom.get_radio_address();
                let speed = eeprom.get_radio_speed();

                let speed_str = match speed {
                    RadioSpeed::R250Kbps => "250K",
                    RadioSpeed::R1Mbps => "1M",
                    RadioSpeed::R2Mbps => "2M",
                };

                let address_str = address.iter().map(|b| format!("{:02X}", b)).collect::<String>();
                let radio_uri = format!("radio://0/{}/{}/{}", channel, speed_str, address_str);

                println!("Read radio config: channel={}, speed={}, address={}", channel, speed, address_str);

                // Disconnect mid-command since we only needed this connection to read EEPROM
                save_and_disconnect(connected_cf.as_ref().unwrap(), preserve_console).await;
                connected_cf.take();

                radio_uri
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

            println!("Selected: {}", selected_uri);
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
                        let available_vars = cf.log.names();
                        let selected_vars = MultiSelect::new("Select variables to log:", available_vars)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No variables selected"))?;
                        selected_vars.join(",")
                      }
                    };


                    modules::log::print(&cf, names.as_str(), var.period as u64, csv).await?;

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
                        let available_vars = cf.param.names();
                        let selected_vars = MultiSelect::new("Select parameters to show:", available_vars)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No parameters selected"))?;
                        selected_vars.join(",")
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
                        let available_vars = cf.param.names();
                        let available_vars: Vec<String> = available_vars
                          .into_iter()
                          .filter(|name| cf.param.is_writable(name).unwrap_or(false))
                          .collect();
                        let selected_vars = MultiSelect::new("Select parameters to set:", available_vars)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No parameters selected"))?;

                        let mut param_map = HashMap::new();
                        for name in selected_vars {
                          let param: Value = cf.param.get(&name).await?;
                          let value: String = inquire::Text::new(&format!("[{}] {:?}:", name, param))
                            .prompt()
                            .map_err(|_| anyhow::anyhow!("No value entered for parameter '{}'", name))?;
                          param_map.insert(name, value);
                        }
                        param_map
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
                        let available_vars = cf.param.names();
                        let mut persistent_vars = Vec::new();
                        for name in available_vars {
                            if cf.param.is_persistent(&name).await? {
                                persistent_vars.push(name);
                            }
                        }
                        let selected_vars = MultiSelect::new("Select parameters to store:", persistent_vars)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No parameters selected"))?;
                        selected_vars.join(",")
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
                        let available_vars = cf.param.names();
                        let mut persistent_vars = Vec::new();
                        for name in available_vars {
                            if cf.param.is_persistent(&name).await? {
                                persistent_vars.push(name);
                            }
                        }
                        let selected_vars = MultiSelect::new("Select parameters to clear:", persistent_vars)
                          .prompt()
                          .map_err(|_| anyhow::anyhow!("No parameters selected"))?;
                        selected_vars.join(",")
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

                    let memories = cf.memory.get_memories(Some(MemoryType::EEPROMConfig));

                    if memories.len() != 1 {
                      bail!("No EEPROMConfig memory found or more than one ({}), exiting!", memories.len());
                    }

                    let mut eeprom_memory = match cf.memory.open_memory::<EEPROMConfigMemory>(memories[0].clone()).await {
                      Some(Ok(m)) => m,
                      Some(Err(e)) => bail!("Could not access EEPROM memory: {}", e),
                      None => bail!("No EEPROM memory found"),
                    };

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
                        println!("Memories:");
                        for mem in memory {
                          let memory_serial = mem.serial.as_ref()
                            .map(|s| format!(" (0x{})", s.iter().map(|b| format!("{:02X}", b)).collect::<String>()))
                            .unwrap_or_default();
                          println!("[{}] {:?} size={}k (0x{:x}/{}){}", mem.memory_id, mem.memory_type, mem.size / 1024, mem.size, mem.size, memory_serial);
                        }
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

                    let protocol_version = cf.platform.protocol_version().await?;
                    let firmware_version = cf.platform.firmware_version().await?;
                    let device_type_name = cf.platform.device_type_name().await?;

                    if csv {
                        println!("field,value");
                        utils::display::csv_row(&["platform", &device_type_name]);
                        utils::display::csv_row(&["firmware", &firmware_version]);
                        utils::display::csv_row(&["crtp_protocol", &protocol_version.to_string()]);
                    } else {
                        println!("Platform\t: {}", device_type_name);
                        println!("Firmware\t: {}", firmware_version);
                        println!("CRTP protocol\t: {}", protocol_version);
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
            match command {
                LocoCommands::Display => {
                    let cf = connect_cf(&mut connected_cf, &link_context, uri.as_str(), toc_cache, args.debug).await?;
                    modules::lps::display(&cf).await?;
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
                    cf.platform.send_arming_request(true).await?;
                    println!("Crazyflie armed!");
                }
                HlCommands::Disarm => {
                    println!("Disarming Crazyflie...");
                    cf.platform.send_arming_request(false).await?;
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
                  let release = match &params.release {
                    Some(Some(r)) => {
                      let labels = utils::firmware::get_release_labels().await?;
                      if !labels.contains(r) {
                        bail!(CliError::NotFound(format!("release '{}'", r)));
                      }
                      Some(r.clone())
                    },
                    Some(None) => {
                      require_arg(non_interactive, "--release <NAME>")?;
                      let labels = utils::firmware::get_release_labels().await?;
                      let selected_release = Select::new("Select a firmware release to flash:", labels)
                        .prompt()
                        .map_err(|_| anyhow::anyhow!("No release selected"))?;
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
                            require_arg(non_interactive, "--bin target=file")?;
                            let selected_target = Select::new(
                              &format!("Select target for [{}]:", k),
                              bootloader::get_hardcoded_list_of_targets()
                            )
                            .prompt()
                            .map_err(|_| anyhow::anyhow!("No binary selected"))?;
                            (selected_target.to_string(), k.to_string())
                          }
                        };
                        result.insert(k, v);
                      }
                    }
                    Some(result)
                  };

                  let platform = if params.cold {
                    // In cold-boot/recovery mode the Crazyflie is not running firmware,
                    // so we cannot connect to query the platform. Use the --platform
                    // flag or ask the user interactively.
                    let resolve_platform = |p: &str| -> Result<String> {
                      match p.to_lowercase().as_str() {
                        "cf21" => Ok("Crazyflie 2.1".to_string()),
                        "cf21bl" => Ok("Crazyflie 2.1 Brushless".to_string()),
                        "bolt11" => Ok("Crazyflie Bolt 1.1".to_string()),
                        "flapper" => Ok("Flapper (Bolt 1.1)".to_string()),
                        "tag" => Ok("Roadrunner 1.0".to_string()),
                        _ => bail!("Unknown platform '{}'. Valid options: cf21, cf21bl, bolt11, flapper, tag", p),
                      }
                    };
                    match &params.platform {
                      Some(p) => resolve_platform(p)?,
                      None => {
                        require_arg(non_interactive, "--platform")?;
                        let platforms = vec![
                          "Crazyflie 2.1",
                          "Crazyflie 2.1 Brushless",
                          "Crazyflie Bolt 1.1",
                          "Flapper (Bolt 1.1)",
                          "Roadrunner 1.0",
                        ];
                        Select::new("Select the platform:", platforms)
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
                  let mut upgrade = utils::firmware::FirmwareUpgrade::new(&platform, &release, &params.zip, &bin_with_selections).await?;

                  let selected_target_and_types = match &params.targets {
                    Some(Some(t)) => t.split(',').map(|s| s.trim().to_string()).collect(),
                    Some(None) => {
                      require_arg(non_interactive, "--targets <list>")?;
                      let available_target_and_types = upgrade.get_target_and_types();

                      let selected_target_and_types = MultiSelect::new("Select targets to flash:", available_target_and_types)
                        .prompt()
                        .map_err(|_| anyhow::anyhow!("No targets selected"))?;
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
