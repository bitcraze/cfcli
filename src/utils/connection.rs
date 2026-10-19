//! Reaching the Crazyflies through `crazyflie-connection-server`: with this computer's
//! Crazyradios (direct, the server runs inside cfcli) or through a connection
//! server, chosen at runtime by `--connection`, `CRAZYFLIE_CONNECTION` or
//! `cfcli settings connection`. The commands ported to the client API work the
//! same in every mode. The others still use crazyflie-lib directly and only
//! work in direct mode.

use anyhow::{anyhow, bail, Context, Result};
use crazyflie_connection_server::{Client, Crazyflie, InProcess, ServerConfig};
use crazyflie_lib::crazyflie_link::LinkContext;
use crazyflie_lib::{Value, ValueType};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::CliError;
use crate::{save_console_history, update_client_completion_cache, Config, ConfigTocCache};

/// The mode when none is set: a server on this computer if one runs, else direct.
pub const DEFAULT_MODE: &str = "auto";

/// The connection mode: `--connection` or `CRAZYFLIE_CONNECTION`, else the setting, else [`DEFAULT_MODE`].
pub fn mode(arg: Option<&str>, config: &Config) -> String {
    arg.or(config.connection.as_deref()).unwrap_or(DEFAULT_MODE).to_string()
}

/// The client connection of one cfcli run, opened by the first command that needs it.
pub struct Connection {
    mode: String,
    toc_cache: ConfigTocCache,
    link_context: Arc<LinkContext>,
    client: Option<Client>,
    cf: Option<Crazyflie>,
}

impl Connection {
    /// `link_context` is cfcli's own, which the server shares in direct mode.
    pub fn new(mode: String, toc_cache: ConfigTocCache, link_context: Arc<LinkContext>) -> Self {
        Connection { mode, toc_cache, link_context, client: None, cf: None }
    }

    /// The client, opened on first use. In direct mode the server runs in
    /// this process, opens a Crazyradio only while it uses it (so other
    /// programs keep the others), uses cfcli's TOC cache and Crazyradios (so
    /// that the commands still done with crazyflie-lib share them) and
    /// disconnects at exit.
    pub async fn client(&mut self) -> Result<&Client> {
        if self.client.is_none() {
            let direct = ServerConfig {
                name: "cfcli".to_string(),
                linger: Duration::ZERO,
                hold_radios: false,
                toc_cache: None,
                link_context: Some(self.link_context.clone()),
            }
            .with_toc_cache(self.toc_cache.clone());
            // cfcli commands are short-lived, so `auto` never serves other programs.
            let client = Client::open_with(&self.mode, InProcess { config: direct, auto_shared: false })
                .await
                .context(CliError::Connection(format!("opening the connection server ({})", self.mode)))?;
            self.client = Some(client);
        }
        Ok(self.client.as_ref().expect("opened above"))
    }

    /// Attach to the Crazyflie at `uri`. The server connects it unless
    /// another program already has.
    pub async fn attach(&mut self, uri: &str, debug: bool) -> Result<&Crazyflie> {
        let start = Instant::now();
        let client = self.client().await?.clone();
        let cf = client.attach(uri).await.context(CliError::Connection(format!("connecting to {}", uri)))?;
        if debug {
            eprintln!("Connection time: {:.2?} ({})", start.elapsed(), client.mode());
        }
        // Completion follows the most recently connected Crazyflie.
        update_client_completion_cache(&cf);
        Ok(self.cf.insert(cf))
    }

    /// Save the console history (`--preserve-console`, direct mode only: a
    /// server keeps the console) and, in direct mode, disconnect.
    pub async fn close(&mut self, preserve_console: bool) {
        if preserve_console {
            if let Some(cf) = self.cf.as_ref().and_then(|cf| cf.lib()) {
                if let Err(e) = save_console_history(&cf).await {
                    eprintln!("Warning: could not save console history: {}", e);
                }
            }
        }
        if let Some(client) = &self.client {
            client.close().await;
        }
        self.cf = None;
        self.client = None;
    }
}

/// Run a connection server until Ctrl-C: it holds this
/// computer's Crazyradios and shares the Crazyflies between the programs
/// attached to it.
pub async fn run(listen: &str, linger: u64, toc_cache: Option<std::path::PathBuf>) -> Result<()> {
    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|_| CliError::InvalidValue(format!("'{}' is not an address like 127.0.0.1:7777", listen)))?;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("crazyflie_connection_server=info")).init();
    // Listen first, so that a second server fails before it claims any Crazyradio
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context(CliError::Connection(format!("listening on {}", addr)))?;
    // On disk, not in cfcli's config file: a long-running server would write
    // back the whole config it loaded, undoing other cfcli runs' changes
    let toc_dir = match toc_cache {
        Some(dir) => dir,
        None => {
            use etcetera::BaseStrategy;
            etcetera::choose_base_strategy()
                .map_err(|e| anyhow!("no cache directory ({}); give one with --toc-cache", e))?
                .cache_dir()
                .join("cf-cli")
                .join("server-toc-cache")
        }
    };
    let toc_cache = crazyflie_connection_server::FileTocCache::new(&toc_dir)
        .context(format!("creating the TOC cache {}", toc_dir.display()))?;
    let config = ServerConfig {
        name: format!("cfcli on {}", gethostname::gethostname().to_string_lossy()),
        linger: Duration::from_secs(linger),
        ..Default::default()
    }
    .with_toc_cache(toc_cache);
    let server = crazyflie_connection_server::Server::new(config).await;
    if server.radios().is_empty() {
        eprintln!("Warning: no Crazyradio could be opened, only USB Crazyflies are reachable");
    }
    println!("Connection server on ws://{} with Crazyradios {:?}. Ctrl-C stops it.", addr, server.radios());
    println!("TOCs are cached in {}", toc_dir.display());
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    crazyflie_connection_server::serve_on(server.clone(), listener, ctrl_c)
        .await
        .context(CliError::Connection(format!("serving on {}", addr)))?;
    server.shutdown().await;
    Ok(())
}

/// Whether the Crazyflies are reached through a connection server outside
/// cfcli: `local`, an address, or `auto` with a server on this computer.
pub async fn through_server(mode: &str) -> bool {
    match mode {
        "direct" | "shared" => false,
        "auto" | "" => crazyflie_connection_server::local_server_running().await,
        _ => true,
    }
}

/// Fail unless a command that uses crazyflie-lib directly can open the
/// Crazyradios: in direct mode, or in auto mode with no server running.
pub async fn require_direct(mode: &str) -> Result<()> {
    match mode {
        "direct" => Ok(()),
        "auto" | "" if !crazyflie_connection_server::local_server_running().await => Ok(()),
        "auto" | "" | "local" => bail!(CliError::Connection(
            "a connection server on this computer holds the Crazyradios, and this command does not \
             go through it yet. Stop the server, or use --connection direct with a Crazyradio it does not hold"
                .to_string()
        )),
        _ => bail!(CliError::Connection(format!(
            "this command does not go through a connection server yet (connection '{}'). Use --connection direct",
            mode
        ))),
    }
}

/// The crazyflie-lib `Crazyflie` behind `cf`, for the parts of a command
/// that the client API does not cover yet. Direct mode only.
pub fn lib(cf: &Crazyflie, what: &str) -> Result<Arc<crazyflie_lib::Crazyflie>> {
    cf.lib().ok_or_else(|| {
        anyhow!(CliError::Connection(format!(
            "{} does not go through a connection server yet. Use --connection direct",
            what
        )))
    })
}

/// The crazyflie-lib type of a param or log variable. The client API names
/// types like the firmware does (`u8`, `f32`, …).
pub fn value_type(type_name: &str) -> Result<ValueType> {
    Ok(match type_name {
        "u8" => ValueType::U8,
        "u16" => ValueType::U16,
        "u32" => ValueType::U32,
        "u64" => ValueType::U64,
        "i8" => ValueType::I8,
        "i16" => ValueType::I16,
        "i32" => ValueType::I32,
        "i64" => ValueType::I64,
        "f16" => ValueType::F16,
        "f32" => ValueType::F32,
        "f64" => ValueType::F64,
        other => bail!("unknown value type '{}'", other),
    })
}

/// The type of a parameter.
pub fn param_type(cf: &Crazyflie, name: &str) -> Result<ValueType> {
    value_type(cf.param.get_type(name)?)
}

/// The type of a log variable.
pub fn log_type(cf: &Crazyflie, name: &str) -> Result<ValueType> {
    value_type(cf.log.get_type(name)?)
}

/// A parameter value from the client API (an `f64`) as a crazyflie-lib
/// `Value` of the parameter's type, so it prints as before. Exact for every
/// type up to 32 bits.
pub fn param_value(cf: &Crazyflie, name: &str, value: f64) -> Result<Value> {
    Ok(Value::from_f64_lossy(param_type(cf, name)?, value))
}

/// Read a parameter as a typed `Value`.
pub async fn get_param(cf: &Crazyflie, name: &str) -> Result<Value> {
    let value = cf.param.get_lossy(name).await?;
    param_value(cf, name, value)
}
