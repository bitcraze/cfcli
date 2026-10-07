//! Shared swarms: swarms kept on the server (`cfcli auth login`), named
//! `<org>/<swarm>`, next to the local ones, which are named `<swarm>`.
//!
//! cfcli keeps a copy of each shared swarm it uses in
//! `synced/<server>/<org>/<swarm>.yaml` next to the cfcli config. That is
//! outside the `swarms` folder, so Swarmkeeper and `import` never take a copy
//! for a local swarm. `synced/<server>/state.json` says which revision each
//! copy is and whether it has changes the server doesn't have yet.
//!
//! With sync on (`cfcli settings sync`, the default) a command checks the
//! server before it uses a shared swarm, which costs a 304 when the copy is
//! current, and writes changes straight to the server. The upload carries
//! `If-Match` with the copy's revision, so nobody's upload is overwritten:
//! when someone else changed the swarm in between, the change is applied
//! again to their version. With sync off, commands use the copies only, and
//! `swarm config pull` and `push` sync them.
//!
//! When the server can't be reached, commands use the copies and keep their
//! changes for the next push, so working with the Crazyflies never waits for
//! the internet.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{header, RequestBuilder, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::store::{self, Store, Swarm};
use crate::error::CliError;
use crate::modules::auth::{self, Credentials};

/// A shared swarm's ID, `<org>/<swarm>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SharedId {
    pub org: String,
    pub swarm: String,
}

impl SharedId {
    /// `<org>/<swarm>`, or None for a local ID (no '/').
    pub fn parse(id: &str) -> Result<Option<Self>> {
        let Some((org, swarm)) = id.split_once('/') else {
            return Ok(None);
        };
        let org_ok = !org.is_empty()
            && org.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !org_ok {
            bail!(CliError::InvalidValue(format!(
                "swarm ID '{}': the organization is lower-case letters, digits and '-'",
                id
            )));
        }
        store::check_id(swarm)?;
        Ok(Some(SharedId { org: org.to_string(), swarm: swarm.to_string() }))
    }
}

impl fmt::Display for SharedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.swarm)
    }
}

/// What cfcli knows about its copy of a shared swarm.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CopyState {
    /// The server's revision the copy is based on. 0 for a swarm created
    /// here while the server couldn't be reached, which isn't on it yet.
    pub revision: i32,
    /// The copy has changes the server doesn't have.
    pub pending: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    swarms: BTreeMap<String, CopyState>,
}

/// A shared swarm as `swarm config list` shows it.
pub struct Listed {
    pub id: SharedId,
    pub name: String,
    pub units: usize,
    pub copy: CopyState,
}

#[derive(Deserialize)]
struct ServerSwarm {
    org: String,
    swarm: String,
    name: String,
    units: usize,
    revision: i32,
}

/// What an upload asks the server to check first.
#[derive(Clone, Copy)]
enum Condition {
    /// Only if the swarm is still at this revision.
    Revision(i32),
    /// Only if the swarm doesn't exist.
    New,
    /// Overwrite whatever is there.
    None,
}

enum Upload {
    Done(i32),
    /// Someone else changed (or created, or deleted) the swarm first.
    Conflict,
    Offline,
}

/// The shared swarms of the server cfcli is signed in to.
pub struct Shared {
    credentials: Credentials,
    /// `synced/<server>` next to the cfcli config.
    dir: PathBuf,
    sync: bool,
    client: reqwest::Client,
    /// The server didn't answer once; don't wait for it again in this command.
    offline: AtomicBool,
}

impl Shared {
    /// None when cfcli isn't signed in. Doesn't contact the server.
    pub fn open(sync: bool) -> Result<Option<Self>> {
        let Some(credentials) = auth::load()? else {
            return Ok(None);
        };
        let config = confy::get_configuration_file_path("cf-cli", None)
            .context("could not find the cfcli config folder")?;
        // "localhost:3000" would be an invalid folder name on Windows.
        let host = auth::host(&credentials.server).replace([':', '/', '\\'], "_");
        let dir = config.with_file_name("synced").join(host);
        let client = reqwest::Client::builder()
            .user_agent(concat!("cfcli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Some(Shared { credentials, dir, sync, client, offline: AtomicBool::new(false) }))
    }

    pub fn host(&self) -> &str {
        auth::host(&self.credentials.server)
    }

    pub fn sync(&self) -> bool {
        self.sync
    }

    // ---- The copies and their state ----

    fn copies(&self, org: &str) -> Store {
        Store::at(self.dir.join(org))
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn state(&self) -> Result<State> {
        let path = self.state_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn save_state(&self, state: &State) -> Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.state_path();
        let tmp = self.dir.join(".state.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(state)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    pub fn copy_state(&self, id: &SharedId) -> Result<Option<CopyState>> {
        Ok(self.state()?.swarms.get(&id.to_string()).copied())
    }

    fn set_copy_state(&self, id: &SharedId, copy: Option<CopyState>) -> Result<()> {
        let mut state = self.state()?;
        match copy {
            Some(copy) => state.swarms.insert(id.to_string(), copy),
            None => state.swarms.remove(&id.to_string()),
        };
        self.save_state(&state)
    }

    /// Keep `swarm` as the copy of `id`.
    fn keep(&self, id: &SharedId, swarm: &Swarm, copy: CopyState) -> Result<()> {
        self.copies(&id.org).save(&id.swarm, swarm)?;
        self.set_copy_state(id, Some(copy))
    }

    fn forget(&self, id: &SharedId) -> Result<()> {
        let copies = self.copies(&id.org);
        if copies.exists(&id.swarm) {
            copies.delete(&id.swarm)?;
        }
        self.set_copy_state(id, None)
    }

    fn has_copy(&self, id: &SharedId) -> bool {
        self.copies(&id.org).exists(&id.swarm)
    }

    /// The copy, without asking the server. For shell completion and the
    /// like, which must never wait for the network.
    pub fn cached(&self, id: &SharedId) -> Result<Swarm> {
        if !self.has_copy(id) {
            bail!(CliError::NotFound(format!("a copy of swarm '{}' on this computer", id)));
        }
        self.copies(&id.org).load(&id.swarm)
    }

    /// The shared swarms there are copies of, without asking the server.
    pub fn cached_ids(&self) -> Vec<SharedId> {
        self.state()
            .map(|state| state.swarms.keys().filter_map(|id| SharedId::parse(id).ok().flatten()).collect())
            .unwrap_or_default()
    }

    // ---- Talking to the server ----

    fn url(&self, id: &SharedId) -> Result<reqwest::Url> {
        auth::endpoint(&self.credentials.server, &format!("/api/v1/swarms/{}/{}", id.org, id.swarm))
    }

    /// Send a request with the key. None when the server can't be reached;
    /// the first time, cfcli says it uses the copies instead.
    async fn send(&self, request: RequestBuilder) -> Result<Option<Response>> {
        self.send_quietly(request, false).await
    }

    async fn send_quietly(&self, request: RequestBuilder, quiet: bool) -> Result<Option<Response>> {
        if self.offline.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match request.bearer_auth(&self.credentials.key).send().await {
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => bail!(CliError::NotFound(format!(
                "a working key: {} no longer accepts '{}'; sign in again with 'cfcli auth login'",
                self.host(),
                self.credentials.key_name
            ))),
            Ok(response) => Ok(Some(response)),
            Err(e) if e.is_connect() || e.is_timeout() => {
                self.offline.store(true, Ordering::Relaxed);
                if !quiet {
                    eprintln!(
                        "Can't reach {}: using the copies of shared swarms on this computer. \
                         Changes are kept and uploaded {}.",
                        self.host(),
                        if self.sync {
                            "when it answers again"
                        } else {
                            "with 'cfcli swarm config push'"
                        }
                    );
                }
                Ok(None)
            }
            Err(e) => Err(e).with_context(|| CliError::Connection(self.host().to_string())),
        }
    }

    /// Like [`Shared::send`], for things that can't be done without the server.
    async fn send_online(&self, request: RequestBuilder, what: &str) -> Result<Response> {
        match self.send_quietly(request, true).await? {
            Some(response) => Ok(response),
            None => bail!(CliError::Connection(format!("{} needs {}, which can't be reached", what, self.host()))),
        }
    }

    /// The swarms on the server, in all the user's organizations.
    async fn server_list(&self) -> Result<Option<Vec<ServerSwarm>>> {
        let url = auth::endpoint(&self.credentials.server, "/api/v1/swarms")?;
        match self.send(self.client.get(url)).await? {
            Some(response) => Ok(Some(auth::answer(&self.credentials.server, response).await?)),
            None => Ok(None),
        }
    }

    /// Download `id` into its copy unless the copy is current. Returns false
    /// when the server can't be reached.
    async fn download(&self, id: &SharedId, force: bool) -> Result<bool> {
        let copy = self.copy_state(id)?.unwrap_or_default();
        let mut request = self.client.get(self.url(id)?);
        if !force && copy.revision > 0 && self.has_copy(id) {
            request = request.header(header::IF_NONE_MATCH, format!("\"{}\"", copy.revision));
        }
        let Some(response) = self.send(request).await? else {
            return Ok(false);
        };
        match response.status() {
            StatusCode::NOT_MODIFIED => Ok(true),
            StatusCode::NOT_FOUND => {
                // Deleted on the server, or no longer visible to the user.
                if copy.pending {
                    bail!(CliError::NotFound(format!(
                        "swarm '{}' on {}, though this computer has changes to it that aren't uploaded; \
                         'cfcli swarm config push --force {}' creates it again with them",
                        id,
                        self.host(),
                        id
                    )));
                }
                self.forget(id)?;
                bail!(CliError::NotFound(format!("swarm '{}' on {}", id, self.host())))
            }
            status if status.is_success() => {
                let revision = etag_revision(&response)?;
                let yaml = response.text().await?;
                let swarm = Swarm::from_yaml(&yaml).with_context(|| format!("swarm '{}' from {}", id, self.host()))?;
                self.keep(id, &swarm, CopyState { revision, pending: false })?;
                Ok(true)
            }
            _ => auth::answer::<serde_json::Value>(&self.credentials.server, response).await.map(|_| true),
        }
    }

    async fn upload(&self, id: &SharedId, swarm: &Swarm, condition: Condition) -> Result<Upload> {
        let mut request = self
            .client
            .put(self.url(id)?)
            .header(header::CONTENT_TYPE, "application/yaml")
            .body(swarm.to_yaml()?);
        request = match condition {
            Condition::Revision(revision) => request.header(header::IF_MATCH, format!("\"{}\"", revision)),
            Condition::New => request.header(header::IF_NONE_MATCH, "*"),
            Condition::None => request,
        };
        let Some(response) = self.send(request).await? else {
            return Ok(Upload::Offline);
        };
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(Upload::Conflict);
        }
        #[derive(Deserialize)]
        struct Uploaded {
            revision: i32,
        }
        let uploaded: Uploaded = auth::answer(&self.credentials.server, response).await?;
        Ok(Upload::Done(uploaded.revision))
    }

    // ---- What the swarm commands do ----

    /// Upload the copy of `id` if it has changes. Fails on a conflict, saying
    /// how to settle it. Returns false when the server can't be reached.
    async fn push_pending(&self, id: &SharedId, force: bool) -> Result<bool> {
        let Some(copy) = self.copy_state(id)? else {
            return Ok(true);
        };
        if !copy.pending {
            return Ok(true);
        }
        let swarm = self.copies(&id.org).load(&id.swarm)?;
        let condition = match (force, copy.revision) {
            (true, _) => Condition::None,
            (false, 0) => Condition::New,
            (false, revision) => Condition::Revision(revision),
        };
        match self.upload(id, &swarm, condition).await? {
            Upload::Done(revision) => {
                self.set_copy_state(id, Some(CopyState { revision, pending: false }))?;
                Ok(true)
            }
            Upload::Offline => Ok(false),
            Upload::Conflict => bail!(CliError::InvalidValue(format!(
                "swarm '{}' changed on {} since this computer's copy ({}). Keep theirs with \
                 'cfcli swarm config pull --force {}', or yours with 'cfcli swarm config push --force {}'",
                id,
                self.host(),
                match copy.revision {
                    0 => "someone created it first".to_string(),
                    revision => format!("revision {}", revision),
                },
                id,
                id
            ))),
        }
    }

    /// The swarm, checked with the server first when sync is on (or when
    /// there's no copy yet).
    pub async fn load(&self, id: &SharedId) -> Result<Swarm> {
        if (self.sync || !self.has_copy(id)) && self.push_pending(id, false).await? {
            self.download(id, false).await?;
        }
        self.cached(id)
    }

    /// Change the swarm. `change` may run more than once: when someone else
    /// uploaded in between, it runs again on their version. With sync off,
    /// or without the server, it changes the copy and keeps it for a push.
    pub async fn change<T>(&self, id: &SharedId, mut change: impl FnMut(&mut Swarm) -> Result<T>) -> Result<T> {
        let mut online = false;
        if self.sync || !self.has_copy(id) {
            online = self.push_pending(id, false).await? && self.download(id, false).await?;
        }
        if online {
            for _ in 0..3 {
                let mut swarm = self.cached(id)?;
                let before = swarm.to_yaml()?;
                let revision = self.copy_state(id)?.unwrap_or_default().revision;
                let result = change(&mut swarm)?;
                if swarm.to_yaml()? == before {
                    return Ok(result);
                }
                match self.upload(id, &swarm, Condition::Revision(revision)).await? {
                    Upload::Done(revision) => {
                        self.keep(id, &swarm, CopyState { revision, pending: false })?;
                        return Ok(result);
                    }
                    // Someone else was first: their version, then the change again.
                    Upload::Conflict => {
                        self.download(id, true).await?;
                    }
                    Upload::Offline => break,
                }
            }
        }
        let mut swarm = self.cached(id)?;
        let before = swarm.to_yaml()?;
        let result = change(&mut swarm)?;
        if swarm.to_yaml()? == before {
            return Ok(result);
        }
        let revision = self.copy_state(id)?.unwrap_or_default().revision;
        self.keep(id, &swarm, CopyState { revision, pending: true })?;
        if !self.sync {
            println!("Changed this computer's copy of '{}'; 'cfcli swarm config push' uploads it", id);
        }
        Ok(result)
    }

    /// Create the swarm on the server. Returns false when it already exists.
    /// Without the server, the copy waits for a push.
    pub async fn create(&self, id: &SharedId, swarm: &Swarm) -> Result<bool> {
        if self.has_copy(id) && self.copy_state(id)?.is_some_and(|c| c.pending) {
            return Ok(false);
        }
        match self.upload(id, swarm, Condition::New).await? {
            Upload::Done(revision) => self.keep(id, swarm, CopyState { revision, pending: false })?,
            Upload::Conflict => return Ok(false),
            Upload::Offline => {
                self.keep(id, swarm, CopyState { revision: 0, pending: true })?;
                println!("Created '{}' on this computer; 'cfcli swarm config push' uploads it", id);
            }
        }
        Ok(true)
    }

    /// Delete the swarm on the server, and the copy. Needs the server,
    /// unless the swarm never got there.
    pub async fn delete(&self, id: &SharedId) -> Result<()> {
        let copy = self.copy_state(id)?;
        if copy.is_some_and(|c| c.revision == 0) {
            return self.forget(id);
        }
        let mut request = self.client.delete(self.url(id)?);
        // With sync off, the user decided on the copy they have: don't delete
        // what someone uploaded since. With sync on, the copy is just the
        // last one used, so the swarm goes whatever its revision.
        if let Some(copy) = copy.filter(|_| !self.sync) {
            request = request.header(header::IF_MATCH, format!("\"{}\"", copy.revision));
        }
        let response = self.send_online(request, "deleting a shared swarm").await?;
        match response.status() {
            status if status.is_success() => self.forget(id),
            StatusCode::NOT_FOUND => {
                self.forget(id)?;
                bail!(CliError::NotFound(format!("swarm '{}' on {}", id, self.host())))
            }
            StatusCode::PRECONDITION_FAILED => bail!(CliError::InvalidValue(format!(
                "swarm '{}' changed on {} since this computer's copy; \
                 'cfcli swarm config pull --force {}' gets the new version",
                id,
                self.host(),
                id
            ))),
            _ => auth::answer::<serde_json::Value>(&self.credentials.server, response).await.map(|_| ()),
        }
    }

    /// The shared swarms: from the server when sync is on (and it answers),
    /// else from the copies. Swarms created here and not uploaded are
    /// included either way.
    pub async fn list(&self) -> Result<Vec<Listed>> {
        let state = self.state()?;
        let server = if self.sync { self.server_list().await? } else { None };
        let mut listed = Vec::new();
        match server {
            Some(server) => {
                for s in server {
                    let id = SharedId { org: s.org, swarm: s.swarm };
                    let copy = state.swarms.get(&id.to_string()).copied().unwrap_or(CopyState {
                        revision: s.revision,
                        pending: false,
                    });
                    listed.push(Listed { id, name: s.name, units: s.units, copy });
                }
                // Copies of swarms gone from the server go too, unless they
                // have changes to upload.
                for (key, copy) in &state.swarms {
                    let Some(id) = SharedId::parse(key)? else { continue };
                    if listed.iter().any(|l| l.id == id) {
                        continue;
                    }
                    if copy.pending {
                        listed.push(self.listed_copy(id, *copy));
                    } else {
                        self.forget(&id)?;
                    }
                }
            }
            None => {
                for (key, copy) in &state.swarms {
                    if let Some(id) = SharedId::parse(key)? {
                        listed.push(self.listed_copy(id, *copy));
                    }
                }
            }
        }
        listed.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(listed)
    }

    fn listed_copy(&self, id: SharedId, copy: CopyState) -> Listed {
        let (name, units) = match self.cached(&id) {
            Ok(swarm) => (swarm.name, swarm.units.len()),
            Err(e) => (format!("(can't read: {:#})", e), 0),
        };
        Listed { id, name, units, copy }
    }

    /// Get the latest version of every shared swarm (or of one). Copies
    /// with changes that aren't uploaded are kept, unless `force`.
    pub async fn pull(&self, only: Option<&SharedId>, force: bool) -> Result<()> {
        let Some(server) = self.server_list().await? else {
            bail!(CliError::Connection(format!("{} can't be reached", self.host())));
        };
        let on_server: Vec<SharedId> = server.into_iter().map(|s| SharedId { org: s.org, swarm: s.swarm }).collect();
        if let Some(only) = only {
            if !on_server.contains(only) {
                bail!(CliError::NotFound(format!("swarm '{}' on {}", only, self.host())));
            }
        }
        let mut kept = 0;
        for id in on_server.iter().filter(|id| only.is_none_or(|only| *id == only)) {
            let copy = self.copy_state(id)?;
            if copy.is_some_and(|c| c.pending) && !force {
                println!("Kept '{}': it has changes that aren't uploaded", id);
                kept += 1;
                continue;
            }
            let before = copy.map(|c| c.revision);
            self.download(id, force).await?;
            let after = self.copy_state(id)?.map(|c| c.revision);
            if before != after {
                println!("Pulled '{}' (revision {})", id, after.unwrap_or_default());
            }
        }
        // Copies of swarms deleted on the server.
        for id in self.cached_ids() {
            if on_server.contains(&id) || only.is_some_and(|only| *only != id) {
                continue;
            }
            let pending = self.copy_state(&id)?.is_some_and(|c| c.pending);
            if pending && !force {
                println!("Kept '{}': it is gone from {}, but has changes that aren't uploaded", id, self.host());
                kept += 1;
            } else {
                self.forget(&id)?;
                println!("Removed '{}': it is gone from {}", id, self.host());
            }
        }
        if kept > 0 {
            println!("Upload kept changes with 'cfcli swarm config push', or drop them with 'pull --force'");
        }
        Ok(())
    }

    /// Upload the changes made while sync was off or the server was out of
    /// reach. `force` overwrites what others uploaded in between.
    pub async fn push(&self, only: Option<&SharedId>, force: bool) -> Result<()> {
        let pending: Vec<SharedId> = self
            .state()?
            .swarms
            .iter()
            .filter(|(_, copy)| copy.pending)
            .filter_map(|(id, _)| SharedId::parse(id).ok().flatten())
            .filter(|id| only.is_none_or(|only| id == only))
            .collect();
        if pending.is_empty() {
            println!("Nothing to push");
            return Ok(());
        }
        let mut failed = Vec::new();
        for id in &pending {
            match self.push_pending(id, force).await {
                Ok(true) => println!(
                    "Pushed '{}' (revision {})",
                    id,
                    self.copy_state(id)?.unwrap_or_default().revision
                ),
                Ok(false) => bail!(CliError::Connection(format!("{} can't be reached", self.host()))),
                Err(e) => {
                    match e.downcast_ref::<CliError>() {
                        Some(CliError::InvalidValue(message)) => println!("{}", message),
                        _ => println!("{:#}", e),
                    }
                    failed.push(id.to_string());
                }
            }
        }
        if !failed.is_empty() {
            bail!(CliError::InvalidValue(format!("not pushed: {}", failed.join(", "))));
        }
        Ok(())
    }
}

/// The revision in a response's ETag, `"N"`.
fn etag_revision(response: &Response) -> Result<i32> {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|etag| etag.to_str().ok())
        .and_then(|etag| etag.trim_start_matches("W/").trim_matches('"').parse().ok())
        .context("the server sent a swarm without its revision")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shared_and_local_ids() {
        assert_eq!(SharedId::parse("lab").unwrap(), None);
        let id = SharedId::parse("bitcraze-lab/cage").unwrap().unwrap();
        assert_eq!((id.org.as_str(), id.swarm.as_str()), ("bitcraze-lab", "cage"));
        assert_eq!(id.to_string(), "bitcraze-lab/cage");
        for bad in ["Lab/cage", "/cage", "lab/", "lab/a/b", "lab/.x", "la b/x"] {
            assert!(SharedId::parse(bad).is_err(), "{}", bad);
        }
    }
}
