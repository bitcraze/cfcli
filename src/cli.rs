// CLI definition (clap derive tree) for cfcli.
//
// This file is `include!`d by both `src/main.rs` (the real binary) and
// `build.rs` (which generates shell-completion scripts from the same
// command tree). It must therefore stay self-contained: NO `use`/`mod`
// statements (it relies on the imports of whichever file includes it) and
// NO dependency on `crazyflie_lib` or other internal modules. The memory
// type is mirrored locally as `MemoryTypeArg`; convert to the lib type via
// `MemoryTypeArg::to_lib` (defined in main.rs).

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
    Type(MemoryTypeArg, Option<usize>),
}

/// Memory type as accepted on the command line. Mirrors
/// `crazyflie_lib::subsystems::memory::MemoryType` but is kept local and
/// free of any lib dependency so the CLI definition can be compiled by the
/// completion-generating build script. Deriving `ValueEnum` also gives shell
/// completion of the type names for free. The explicit `value(name = ...)`
/// attributes preserve the exact (case-sensitive) names the CLI has always
/// accepted. Convert to the lib type with [`MemoryTypeArg::to_lib`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum MemoryTypeArg {
    #[value(name = "EEPROMConfig")]
    EepromConfig,
    #[value(name = "OneWire")]
    OneWire,
    #[value(name = "DriverLed")]
    DriverLed,
    #[value(name = "Loco")]
    Loco,
    #[value(name = "Trajectory")]
    Trajectory,
    #[value(name = "Loco2")]
    Loco2,
    #[value(name = "Lighthouse")]
    Lighthouse,
    #[value(name = "MemoryTester")]
    MemoryTester,
    #[value(name = "MicroSD")]
    MicroSd,
    #[value(name = "DriverLedTiming")]
    DriverLedTiming,
    #[value(name = "App")]
    App,
    #[value(name = "DeckMemory")]
    DeckMemory,
    #[value(name = "DeckCtrlDFU")]
    DeckCtrlDfu,
    #[value(name = "DeckCtrl")]
    DeckCtrl,
    #[value(name = "DeckMultiranger")]
    DeckMultiranger,
    #[value(name = "DeckPaa3905")]
    DeckPaa3905,
}

fn parse_memory_type(s: &str) -> Result<MemoryTypeArg, String> {
    MemoryTypeArg::from_str(s, true).map_err(|_| {
        format!(
            "Unknown memory type '{}'. Valid types: EEPROMConfig, OneWire, DriverLed, \
             Loco, Trajectory, Loco2, Lighthouse, MemoryTester, MicroSD, DriverLedTiming, \
             App, DeckMemory, DeckCtrlDFU, DeckCtrl, DeckMultiranger, DeckPaa3905",
            s
        )
    })
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

    /// Lighthouse positioning system configuration
    Lh {
        #[clap(subcommand)]
        command: LighthouseCommands,
    },

    /// Generate a shell completion script (printed to stdout)
    Completions {
        /// Shell to generate the completion script for
        #[clap(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompletionKind {
    /// All parameter names (for `param get`)
    ParamNames,
    /// Writable parameter names only (for `param set`/`store`/`clear`)
    ParamNamesWritable,
    /// Log variable names (for `log print`)
    LogNames,
    /// Firmware flash target names (for `bootload flash --targets`/`--bin`)
    FlashTargets,
    /// EEPROM config setting names (for `config set`)
    ConfigKeys,
}

#[derive(Debug, Subcommand)]
enum LighthouseCommands {
    /// Base station geometry and calibration configuration
    Config {
        #[clap(subcommand)]
        command: LighthouseConfigCommands,
    },
}

#[derive(Debug, Subcommand)]
enum LighthouseConfigCommands {
    /// Display lighthouse configuration in human-readable form
    Display(LighthouseDisplayParameters),
    /// Read lighthouse configuration as YAML (to file or stdout)
    Read(LighthouseReadParameters),
    /// Write lighthouse configuration from YAML (from file or stdin)
    Write(LighthouseWriteParameters),
}

#[derive(Debug, Args)]
struct LighthouseDisplayParameters {
    /// YAML file to display (reads from Crazyflie if omitted)
    #[clap(long, short = 'i', value_hint = ValueHint::FilePath)]
    input: Option<String>,
}

#[derive(Debug, Args)]
struct LighthouseWriteParameters {
    /// YAML file to read configuration from (reads stdin if omitted)
    #[clap(long, short = 'i', value_hint = ValueHint::FilePath)]
    input: Option<String>,
}

#[derive(Debug, Args)]
struct LighthouseReadParameters {
    /// YAML file to write configuration to (writes to stdout if omitted)
    #[clap(long, short = 'o', value_hint = ValueHint::FilePath)]
    output: Option<String>,
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
    #[clap(long, short = 'i', value_hint = ValueHint::FilePath)]
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
  #[clap(long, value_hint = ValueHint::FilePath)]
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
    #[clap(value_parser, value_hint = ValueHint::FilePath)]
    input: String,
    /// Probe index (defaults to interactive selection if more than one debugger is connected)
    #[clap(long, short = 'p')]
    probe_idx: Option<usize>,
}

#[derive(Debug, Args)]
struct DeckBingenParameters {
    /// Input file (in yaml format) containing the full configuration
    #[clap(value_parser, value_hint = ValueHint::FilePath)]
    input: String,
    /// File to save the read raw binary data into
    #[clap(long, short = 'o', value_hint = ValueHint::FilePath)]
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
    #[clap(long, short = 'i', value_hint = ValueHint::FilePath)]
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
    #[clap(value_parser, value_hint = ValueHint::FilePath)]
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
    #[clap(long, short = 'o', value_hint = ValueHint::FilePath)]
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
    #[clap(long, short = 'i', value_hint = ValueHint::FilePath)]
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
