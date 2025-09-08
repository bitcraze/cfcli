use crate::utils::deckctrl::DeckConfig;
use clap::{Args, Parser, Subcommand};
use futures::StreamExt;
use probe_rs::probe::list::Lister;
use probe_rs::{
    flashing::{DownloadOptions},
    Permissions,
};
use serde::{Deserialize, Serialize};
use std::{io::Write, process};

pub mod modules {
    pub mod log;
    pub mod param;
}

pub mod utils {
    pub mod deckctrl;
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CliArgs {
    /// Specify address
    #[clap(short, long, value_parser, default_value_t=String::from("E7E7E7E7E7"))]
    address: String,

    #[clap(subcommand)]
    command: Commands,
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
    /// Various supporting utilities for the Crazyflie and its ecosystem
    Util {
        #[clap(subcommand)]
        command: UtilCommands,
    },

    /// List the Crazyflies found while scanning (on the selected address)
    Scan,

    /// Scan for Crazyflies and select which one to save for later interactions
    Select,

    /// Print the console text from a Crazyflie
    Console,
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
enum DeckControlCommands {
    /// Generate the configuration binary for the top page
    Bingen(DeckBingenParameters),
    /// Flash the configuration binary to the deck
    Binflash(DeckBinflashParameters),
}

#[derive(Debug, Args)]
struct DeckBinflashParameters {
    /// Input file (in yaml format) containing the full configuration
    #[clap(value_parser)]
    input: String,
    /// Probe index
    #[clap(value_parser, default_value_t = -1)]
    probe_idx: i8,
}

#[derive(Debug, Args)]
struct DeckBingenParameters {
    /// Input file (in yaml format) containing the full configuration
    #[clap(value_parser)]
    input: String,
    /// Binary output for writing directly to flash
    #[clap(value_parser, default_value = "dev-deck.bin")]
    output: String,
}


#[derive(Debug, Args)]
struct VariableName {
    /// Name of variable
    #[clap(value_parser)]
    name: String,
}

#[derive(Debug, Args)]
struct VariableNameAndValue {
    /// Name of variable
    #[clap(value_parser)]
    name: String,
    /// Value to set
    #[clap(value_parser)]
    value: String,
}

#[derive(Debug, Args)]
struct VariablesAndPeriod {
    /// Comma-separated list of variable names
    #[clap(value_parser)]
    names: String,
    /// The period in milliseconds to log at (default 100ms)
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

    cache: Vec<(String, String)>,
    auto_complete_cache: Option<LatestCache>,
}

impl Default for Config {
    fn default() -> Self {
        println!("No configuration found, loading default values");
        Config {
            uri: "".to_string(),
            auto_complete_cache: None,
            cache: Vec::new(),
        }
    }
}

// fn update_cache(config: &mut Config, cf: &Crazyflie) -> Result<(), Box<dyn std::error::Error>> {

//   let mut auto_complete_cache = LatestCache {
//     log: Vec::new(),
//     param: Vec::new()
//   };

//   for name in cf.log.names() {
//     auto_complete_cache.log.push(LatestCachedLogVariable {
//       name: name.clone()
//     });
//   }

//   for name in cf.param.names() {
//     auto_complete_cache.param.push(LatestCachedParameter {
//       name: name.clone(),
//       readonly: !cf.param.is_writable(&name)?
//     });
//   }

//   config.auto_complete_cache = Some(auto_complete_cache);

//   let cache = cf.get_caches();

//   for entry in cache {
//     let existing_entry = config.cache.iter_mut().find(|x| x.0 == entry.0);
//     if existing_entry.is_none() {
//       config.cache.push(entry);
//     }

//   }

//   confy::store("cf-cli", config).unwrap_or_else(|err| {
//     println!("Could not save configuration: {:?}", err);
//   });

//   Ok(())
// }

// Example scans for Crazyflies, connect the first one and print the log and param variables TOC.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = CliArgs::parse();

    let mut config: Config = confy::load("cf-cli", None).unwrap_or_else(|err| {
        println!("Could not load config file: {:?}", err);
        process::exit(1);
    });

    let link_context = crazyflie_link::LinkContext::new();

    let cf_address: [u8; 5] = match u64::from_str_radix(&args.address.replace("0x", ""), 16) {
        Ok(a) if a <= 0xFFFFFFFFFF => {
            a.to_be_bytes()[3..].try_into().expect("Could not convert u64 to [u8; 5]")
        }
        Ok(_) => {
          return Err("Invalid address, please provide a valid 5 byte hexadecimal address".into());
        }
        Err(_) => {
            return Err("Invalid address, please provide a valid 5 byte hexadecimal address".into());
        }
    };

    match &args.command {
        Commands::Scan => {
            // Scan for Crazyflies on the default address
            let found = link_context.scan(cf_address).await?;

            for uri in found {
                println!("> {}", uri);
            }
        }
        Commands::Select => {
            // Scan for Crazyflies on the default address
            let found = link_context.scan(cf_address).await?;

            if found.is_empty() {
                println!("No Crazyflies found");
                return Ok(());
            }

            for (idx, uri) in found.clone().into_iter().enumerate() {
                println!("[{}] {}", idx, uri);
            }

            let mut selected_uri: Option<String> = None;

            while selected_uri.is_none() {
                print!("> ");
                std::io::stdout()
                    .flush()
                    .expect("Could not flush console output");
                let mut input = String::new();
                std::io::stdin()
                    .read_line(&mut input)
                    .expect("Could not read input");

                selected_uri = match input.trim().parse::<usize>() {
                    Ok(idx) => {
                        if idx < found.len() {
                            Some(found[idx].clone())
                        } else {
                            println!("Invalid index, please try again");
                            None
                        }
                    }
                    Err(_) => {
                        println!("Invalid input, please try again");
                        None
                    }
                };
            }

            let selected_uri = selected_uri.unwrap();

            config.uri = selected_uri.clone();

            confy::store("cf-cli", None, config).unwrap_or_else(|err| {
                println!("Could not save configuration: {:?}", err);
            });

            println!("Saved new default URI: {}", selected_uri.clone());
        }
        Commands::Console => {
            println!("Connecting to {} ...", config.uri);

            let cf = crazyflie_lib::Crazyflie::connect_from_uri(
                &link_context,
                config.uri.as_str(),
            )
            .await?;

            // update_cache(&mut config, &cf).expect("Could not populate last used cache");

            let mut console_stream = cf.console.line_stream().await;

            while let Some(line) = console_stream.next().await {
                println!("{}", line);
            }

            cf.disconnect().await;
        }
        Commands::Log { command } => {
            match command {
                LogCommands::List => {
                    println!("Connecting to {} ...", config.uri);

                    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
                        &link_context,
                        config.uri.as_str(),
                    )
                    .await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::log::list(&cf).await?;

                    cf.disconnect().await;
                }
                LogCommands::Print(var) => {
                    println!("Connecting to {} ...", config.uri);

                    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
                        &link_context,
                        config.uri.as_str(),
                    )
                    .await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::log::print(&cf, var.names.as_str(), var.period as u64).await?;

                    cf.disconnect().await;
                }
            }
        }
        Commands::Param { command } => {
            match command {
                ParamCommands::List => {
                    println!("Connecting to {} ...", config.uri);

                    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
                        &link_context,
                        config.uri.as_str(),
                    )
                    .await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::param::list(&cf).await?;
                }
                ParamCommands::Get(var) => {
                    println!("Connecting to {} ...", config.uri);

                    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
                        &link_context,
                        config.uri.as_str(),
                    )
                    .await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::param::get(&cf, &var.name).await?;
                }
                ParamCommands::Set(var) => {
                    println!("Connecting to {} ...", config.uri);

                    let cf = crazyflie_lib::Crazyflie::connect_from_uri(
                        &link_context,
                        config.uri.as_str(),
                    )
                    .await?;

                    // update_cache(&mut config, &cf).expect("Could not populate last used cache");

                    modules::param::set(&cf, &var.name, &var.value).await?;
                }
            }
        }
        Commands::Util { command } => {
            match command {
                UtilCommands::DeckCtrl { command } => {
                    match command {
                        DeckControlCommands::Bingen(params) => {
                            println!(
                                "Generating deck binary from {} to {}",
                                params.input, params.output
                            );
                            let deck_config = DeckConfig::from_yaml(params.input.clone())?;
                            let bytes = deck_config.to_bytes();
                            // Write bytes to file
                            std::fs::write(&params.output, &bytes)?;

                            // Print bytes as hex with 16 chars in each row
                            println!("Generated binary ({} bytes):", bytes.len());
                            for (i, byte) in bytes.iter().enumerate() {
                                if i % 16 == 0 {
                                    print!("{:08x}: ", i);
                                }
                                print!("{:02x} ", byte);
                                if (i + 1) % 16 == 0 {
                                    println!();
                                }
                            }
                            if bytes.len() % 16 != 0 {
                                println!();
                            }
                        }
                        DeckControlCommands::Binflash(params) => {
                            println!("Generating deck binary from {}", params.input);
                            let deck_config = DeckConfig::from_yaml(params.input.clone())?;
                            let bytes = deck_config.to_bytes();

                            let lister = Lister::new();
                            let probes = lister.list_all();

                            let mut probe_idx = params.probe_idx;
                            if probe_idx < 0 && probes.len() == 1 {
                                probe_idx = 0;
                            } else if probe_idx < 0 || probe_idx >= probes.len() as i8 {
                              println!("Multiple probes found, please select which one to use:");
                              for (i, p) in probes.iter().enumerate() {
                                  println!("[{}] {}", i, p.identifier);
                              }
                              process::exit(1);
                            }

                            println!("Flashing deck binary to probe #{} ...", probe_idx);

                            if probes.is_empty() {
                                println!("No probes found, cannot flash deck");
                                process::exit(1);
                            }

                            let address = 0x08000000 + 1024 * 30;
                            let probe = probes[params.probe_idx as usize].open()?;
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
        
    }

    Ok(())
}
