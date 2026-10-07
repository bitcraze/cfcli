//! `cfcli auth`: sign in to the server that shares swarms, sign out, and show
//! who cfcli is signed in as.
//!
//! Signing in goes through the browser, as `gcloud auth login` does: OAuth 2.0
//! authorization code with PKCE (RFC 7636) and a loopback redirect
//! (RFC 8252). cfcli listens on 127.0.0.1 and opens the server's
//! `/cli/authorize` page; once the user allows cfcli there, the browser brings
//! a one-time code back, and cfcli exchanges it and its PKCE verifier for an
//! API key. The key is kept in `credentials.json` next to the cfcli config,
//! readable only by the user, and is listed on the server's API keys page as
//! "cfcli on <computer>", where it can be revoked.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngExt;
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::error::CliError;
use crate::AuthCommands;

/// The name cfcli signs in with, shown to the user in the browser.
const CLIENT: &str = "cfcli";
/// How long to wait for the user to allow cfcli in the browser.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What `cfcli auth login` stores.
#[derive(Debug, Serialize, Deserialize)]
pub struct Credentials {
    /// The server, without a trailing slash, e.g. `https://arc.bitcraze.io`.
    pub server: String,
    pub key: String,
    /// The key's name on the server, e.g. "cfcli on lab-pc".
    pub key_name: String,
}

pub(crate) async fn run(command: &AuthCommands) -> Result<()> {
    match command {
        AuthCommands::Login { server, no_browser } => login(server, *no_browser).await,
        AuthCommands::Logout => logout().await,
        AuthCommands::Status => status().await,
    }
}

// ---- Stored credentials ----

fn credentials_path() -> Result<PathBuf> {
    let config = confy::get_configuration_file_path("cf-cli", None)
        .context("could not find the cfcli config folder")?;
    Ok(config.with_file_name("credentials.json"))
}

/// The stored credentials, if cfcli is signed in.
pub fn load() -> Result<Option<Credentials>> {
    let path = credentials_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let credentials = serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(credentials))
}

/// Store the credentials, readable by the user only: the key acts as them.
fn save(credentials: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(credentials)?;
    write_private(&path, json.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // The mode only applies to a new file.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents)
}

/// On Windows the config folder is in the user's profile, which only they
/// can read.
#[cfg(not(unix))]
fn write_private(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

fn forget() -> Result<()> {
    let path = credentials_path()?;
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

// ---- Talking to the server ----

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("cfcli/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()?)
}

/// `path` on the server.
pub(crate) fn endpoint(server: &str, path: &str) -> Result<Url> {
    Url::parse(&format!("{server}{path}"))
        .map_err(|_| CliError::InvalidValue(format!("server '{}' is not a URL", server)).into())
}

/// The server's JSON answer, or its `{"error"}` as an error.
pub(crate) async fn answer<T: serde::de::DeserializeOwned>(server: &str, response: reqwest::Response) -> Result<T> {
    let status = response.status();
    if status.is_success() {
        return response.json().await.with_context(|| format!("reading the answer from {}", server));
    }
    #[derive(Deserialize)]
    struct Problem {
        error: String,
    }
    match response.json::<Problem>().await {
        Ok(problem) => bail!("{} answered: {}", server, problem.error),
        Err(_) => bail!("{} answered {}", server, status),
    }
}

#[derive(Deserialize)]
struct UserInfo {
    name: String,
    orgs: Vec<OrgInfo>,
}

#[derive(Deserialize)]
struct OrgInfo {
    slug: String,
    name: String,
    role: String,
}

/// Who a key belongs to, or None if the server no longer accepts it.
async fn whoami(credentials: &Credentials) -> Result<Option<UserInfo>> {
    let server = &credentials.server;
    let response = http()?
        .get(endpoint(server, "/api/v1/user")?)
        .bearer_auth(&credentials.key)
        .send()
        .await
        .with_context(|| CliError::Connection(server.clone()))?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    Ok(Some(answer(server, response).await?))
}

/// Revoke the key on the server. A key that is already revoked is fine.
async fn revoke(credentials: &Credentials) -> Result<()> {
    let server = &credentials.server;
    let response = http()?
        .delete(endpoint(server, "/api/v1/key")?)
        .bearer_auth(&credentials.key)
        .send()
        .await
        .with_context(|| CliError::Connection(server.clone()))?;
    match response.status() {
        status if status.is_success() => Ok(()),
        StatusCode::UNAUTHORIZED => Ok(()),
        _ => answer::<serde_json::Value>(server, response).await.map(|_| ()),
    }
}

pub(crate) fn host(server: &str) -> &str {
    server.split("://").nth(1).unwrap_or(server)
}

fn print_orgs(server: &str, orgs: &[OrgInfo]) {
    if orgs.is_empty() {
        println!(
            "You aren't in an organization yet: create one at {}, or ask an admin to invite you.",
            server
        );
        return;
    }
    println!("Swarms are shared in:");
    for org in orgs {
        println!("  {} ({}, {})", org.name, org.slug, org.role);
    }
}

// ---- Signing in ----

/// 32 random bytes as URL-safe base64 (43 characters).
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes[..]);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The PKCE S256 challenge for a verifier.
fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// This computer's name, for the key's name on the server.
fn device_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().trim().to_string();
    if name.is_empty() {
        "this computer".to_string()
    } else {
        name.chars().take(64).collect()
    }
}

/// What the browser brought back.
enum Callback {
    Code(String),
    Cancelled,
}

async fn login(server: &str, no_browser: bool) -> Result<()> {
    let server = server.trim_end_matches('/').to_string();
    if !server.starts_with("https://") && !server.starts_with("http://") {
        bail!(CliError::InvalidValue(format!("server '{}' must start with https:// or http://", server)));
    }
    let verifier = random_token();
    let state = random_token();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("listening for the browser on 127.0.0.1")?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());

    let mut url = endpoint(&server, "/cli/authorize")?;
    url.query_pairs_mut()
        .append_pair("client", CLIENT)
        .append_pair("device", &device_name())
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("state", &state)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256");

    if no_browser || open::that_detached(url.as_str()).is_err() {
        println!("Open this link in a browser on this computer to sign in:\n\n  {}\n", url);
    } else {
        println!("The sign-in page is open in your browser. If it isn't, open this link:\n\n  {}\n", url);
    }
    println!("Waiting for you to allow cfcli in the browser...");

    let (mut browser, callback) = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_browser(&listener, &state))
        .await
        .map_err(|_| CliError::Timeout("waiting for the sign-in in the browser".to_string()))??;
    let done = format!("{}/cli/done?client={}", server, CLIENT);
    let code = match callback {
        Callback::Code(code) => code,
        Callback::Cancelled => {
            redirect(&mut browser, &format!("{}&cancelled", done)).await;
            bail!("the sign-in was cancelled in the browser");
        }
    };

    let signed_in = async {
        #[derive(Deserialize)]
        struct Token {
            key: String,
            name: String,
        }
        let response = http()?
            .post(endpoint(&server, "/api/v1/cli/token")?)
            .json(&json!({ "code": code, "code_verifier": verifier, "redirect_uri": redirect_uri }))
            .send()
            .await
            .with_context(|| CliError::Connection(server.clone()))?;
        let token: Token = answer(&server, response).await?;
        let credentials = Credentials { server: server.clone(), key: token.key, key_name: token.name };
        let user = whoami(&credentials)
            .await?
            .context("the server didn't accept the key it just made")?;
        anyhow::Ok((credentials, user))
    }
    .await;
    let (credentials, user) = match signed_in {
        Ok(signed_in) => signed_in,
        Err(e) => {
            respond(&mut browser, "500 Internal Server Error", &format!("Signing in cfcli failed: {:#}\n\nGo back to cfcli.\n", e)).await;
            return Err(e);
        }
    };
    redirect(&mut browser, &done).await;

    // A new sign-in replaces the old one; its key isn't needed any more.
    if let Ok(Some(old)) = load() {
        if old.key != credentials.key && revoke(&old).await.is_err() {
            println!(
                "Couldn't revoke the previous key, '{}'. Revoke it at {}/keys.",
                old.key_name, old.server
            );
        }
    }
    save(&credentials)?;

    println!("\nSigned in to {} as {}.", host(&server), user.name);
    print_orgs(&server, &user.orgs);
    println!(
        "cfcli's key is listed as '{}' at {}/keys, where you can revoke it.",
        credentials.key_name, server
    );
    Ok(())
}

/// Take requests until the browser comes back to `/callback` with this
/// sign-in's state. Anything else (a favicon, an old tab) gets an answer and
/// is ignored.
async fn wait_for_browser(listener: &TcpListener, state: &str) -> Result<(TcpStream, Callback)> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        // Browsers sometimes open a connection and send nothing (preconnect),
        // which mustn't hold up the real request.
        let Ok(Some(target)) = tokio::time::timeout(Duration::from_secs(5), read_request_target(&mut stream)).await
        else {
            continue;
        };
        let Ok(url) = Url::parse(&format!("http://127.0.0.1{}", target)) else {
            respond(&mut stream, "400 Bad Request", "Not a sign-in request.\n").await;
            continue;
        };
        if url.path() != "/callback" {
            respond(&mut stream, "404 Not Found", "Not found.\n").await;
            continue;
        }
        let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        if param("state").as_deref() != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                "This page belongs to another sign-in. Use the newest link cfcli printed.\n",
            )
            .await;
            continue;
        }
        if let Some(code) = param("code") {
            return Ok((stream, Callback::Code(code)));
        }
        if param("error").is_some() {
            return Ok((stream, Callback::Cancelled));
        }
        respond(&mut stream, "400 Bad Request", "The sign-in came back without a code.\n").await;
    }
}

/// The target of the HTTP request on `stream`, e.g. `/callback?code=...`.
/// Reads the request line and the headers; there is no body.
async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.ok()?;
    let target = line.split_whitespace().nth(1)?.to_string();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).await.ok()? == 0 || header.trim().is_empty() {
            break;
        }
    }
    Some(target)
}

async fn respond(stream: &mut TcpStream, status: &str, text: &str) {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        text.len(),
        text
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn redirect(stream: &mut TcpStream, location: &str) {
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

// ---- Signing out and status ----

async fn logout() -> Result<()> {
    let Some(credentials) = load()? else {
        println!("cfcli isn't signed in.");
        return Ok(());
    };
    match revoke(&credentials).await {
        Ok(()) => println!(
            "Signed out of {}; the key '{}' is revoked.",
            host(&credentials.server),
            credentials.key_name
        ),
        Err(e) => println!(
            "Signed out here, but couldn't revoke the key ({:#}). Revoke '{}' at {}/keys.",
            e, credentials.key_name, credentials.server
        ),
    }
    forget()
}

async fn status() -> Result<()> {
    let Some(credentials) = load()? else {
        bail!(CliError::NotFound("a sign-in; sign in with 'cfcli auth login'".to_string()));
    };
    let Some(user) = whoami(&credentials).await? else {
        bail!(CliError::NotFound(format!(
            "a working key: {} no longer accepts '{}'; sign in again with 'cfcli auth login'",
            host(&credentials.server),
            credentials.key_name
        )));
    };
    println!(
        "Signed in to {} as {}, with the key '{}'.",
        host(&credentials.server),
        user.name,
        credentials.key_name
    );
    print_orgs(&credentials.server, &user.orgs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc_7636_example() {
        // Appendix B of RFC 7636.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn tokens_are_url_safe_and_differ() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[tokio::test]
    async fn waits_for_its_own_callback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
            let get = |path: &str| client.get(format!("http://127.0.0.1:{}{}", port, path)).send();
            assert_eq!(get("/favicon.ico").await.unwrap().status(), 404);
            assert_eq!(get("/callback?code=old&state=other").await.unwrap().status(), 400);
            get("/callback?code=abc&state=mine").await.unwrap().status()
        });
        let (mut stream, callback) = wait_for_browser(&listener, "mine").await.unwrap();
        assert!(matches!(callback, Callback::Code(code) if code == "abc"));
        redirect(&mut stream, "http://example.com/done").await;
        drop(stream);
        assert_eq!(browser.await.unwrap(), 302);
    }
}
