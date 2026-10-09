//! Documents cfcli keeps: swarms and lighthouse configurations. Each kind is
//! a YAML file, kept locally in a folder next to the cfcli config (`swarms`,
//! `lighthouse`) and named `<id>`, and, when cfcli is signed in (`cfcli auth
//! login`), also shared on the server, named `<org>/<id>`. Every command
//! takes either kind of ID.
//!
//! cfcli keeps a copy of each shared document it uses in
//! `synced/<server>/<kind>/<org>/<id>.yaml` next to the cfcli config. That
//! is outside the local folders, so Swarmkeeper and `import` never take a
//! copy for a local document. `synced/<server>/state.json` says which
//! revision each copy is and whether it has changes the server doesn't have
//! yet.
//!
//! With sync on (`cfcli settings sync`, the default) a command checks the
//! server before it uses a shared document, which costs a 304 when the copy
//! is current, and writes changes straight to the server. The upload carries
//! `If-Match` with the copy's revision, so nobody's upload is overwritten:
//! when someone else changed the document in between, the change is applied
//! again to their version. With sync off, commands use the copies only, and
//! `pull` and `push` sync them.
//!
//! When the server can't be reached, commands use the copies and keep their
//! changes for the next push, so working with the Crazyflies never waits for
//! the internet.
//!
//! The `<org>` in an ID is the user's own ID for the organization, which they
//! can change on the server. `synced/<server>/orgs.json` remembers the ID of
//! each organization (the server lists their UUIDs, which never change), so
//! when one changes, the copies of every kind, the selected swarm and the
//! swarms' links to lighthouse configurations move to the new ID, and the
//! old ID says what the new one is.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{header, RequestBuilder, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::swarm::store::Swarm;
use crate::error::CliError;
use crate::modules::auth::{self, Credentials};
use crate::Config;

/// One kind of document.
pub trait Document: Sized {
    /// The local folder, the folder of the synced copies, the key in the
    /// state file and the server's API path: `swarms`.
    const FOLDER: &'static str;
    /// What one is called in messages: `swarm`.
    const NOUN: &'static str;
    /// The commands that handle them, for hints: `swarm config`.
    const COMMAND: &'static str;
    /// The item count in the server's list: `units`.
    const SERVER_COUNT: &'static str;

    fn from_yaml(yaml: &str) -> Result<Self>;
    fn to_yaml(&self) -> Result<String>;
    /// The name (may be empty) and item count, for lists.
    fn summary(&self) -> (String, usize);

    /// Change organization `old` to `new` in the documents this one names
    /// (a swarm its lighthouse configuration). Returns whether it changed.
    fn rename_org(&mut self, _old: &str, _new: &str) -> bool {
        false
    }
}

/// IDs are file names: letters, digits, '-', '_' and '.', not first.
pub fn check_id<D: Document>(id: &str) -> Result<()> {
    if !is_id(id) {
        bail!(CliError::InvalidValue(format!(
            "{} ID '{}': use letters, digits, '-', '_' and '.' (not first)",
            D::NOUN,
            id
        )));
    }
    Ok(())
}

fn is_id(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

// ---- Local documents ----

/// A folder of documents of one kind.
pub struct Store<D> {
    dir: PathBuf,
    kind: PhantomData<fn() -> D>,
}

impl<D: Document> Store<D> {
    /// The local documents, next to the cfcli config file.
    pub fn open() -> Result<Self> {
        let config = confy::get_configuration_file_path("cf-cli", None)
            .context("could not find the cfcli config folder")?;
        Ok(Store::at(config.with_file_name(D::FOLDER)))
    }

    /// A store in another folder: the copies of shared documents, and tests.
    pub fn at(dir: PathBuf) -> Self {
        Store { dir, kind: PhantomData }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.yaml", id))
    }

    pub fn exists(&self, id: &str) -> bool {
        self.path(id).is_file()
    }

    /// The IDs of all stored documents, sorted.
    pub fn ids(&self) -> Result<Vec<String>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.dir.display())),
        };
        let mut ids: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
            .filter_map(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
            .filter(|id| is_id(id))
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// The file exactly as stored.
    pub fn read_raw(&self, id: &str) -> Result<String> {
        let path = self.path(id);
        match std::fs::read_to_string(&path) {
            Ok(yaml) => Ok(yaml),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(CliError::NotFound(format!(
                "{} '{}' (see 'cfcli {} list')",
                D::NOUN,
                id,
                D::COMMAND
            ))),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn load(&self, id: &str) -> Result<D> {
        D::from_yaml(&self.read_raw(id)?).with_context(|| format!("in {}", self.path(id).display()))
    }

    /// Write a document, replacing any old file in one step.
    pub fn save(&self, id: &str, document: &D) -> Result<()> {
        check_id::<D>(id)?;
        std::fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.path(id);
        let tmp = self.dir.join(format!(".{}.yaml.tmp", id));
        std::fs::write(&tmp, document.to_yaml()?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!(CliError::NotFound(format!("{} '{}'", D::NOUN, id)))
            }
            Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
        }
    }
}

// ---- Shared documents ----

/// A shared document's ID, `<org>/<id>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SharedId {
    pub org: String,
    pub name: String,
}

impl SharedId {
    /// `<org>/<id>`, or None for a local ID (no '/').
    pub fn parse(id: &str) -> Result<Option<Self>> {
        let Some((org, name)) = id.split_once('/') else {
            return Ok(None);
        };
        let org_ok = !org.is_empty() && org.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !org_ok {
            bail!(CliError::InvalidValue(format!(
                "ID '{}': the organization is lower-case letters, digits and '-'",
                id
            )));
        }
        if !is_id(name) {
            bail!(CliError::InvalidValue(format!(
                "ID '{}': after the organization, use letters, digits, '-', '_' and '.' (not first)",
                id
            )));
        }
        Ok(Some(SharedId { org: org.to_string(), name: name.to_string() }))
    }
}

impl fmt::Display for SharedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.name)
    }
}

/// What cfcli knows about its copy of a shared document.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CopyState {
    /// The server's revision the copy is based on. 0 for a document created
    /// here while the server couldn't be reached, which isn't on it yet.
    pub revision: i32,
    /// The copy has changes the server doesn't have.
    pub pending: bool,
}

/// `state.json`: the copies of each kind, by `<org>/<id>`.
type State = BTreeMap<String, BTreeMap<String, CopyState>>;

/// A shared document as `list` shows it.
pub struct Listed {
    pub id: SharedId,
    pub name: String,
    pub count: usize,
    pub copy: CopyState,
}

/// A document in the server's list.
struct ServerItem {
    id: SharedId,
    /// Never changes, unlike the user's ID for the organization (`id.org`).
    /// Servers before per-user IDs don't send it.
    org_uuid: Option<String>,
    name: String,
    count: usize,
    revision: i32,
}

/// `orgs.json`: the user's IDs for their organizations.
#[derive(Debug, Default, Serialize, Deserialize)]
struct OrgIds {
    /// The ID of each organization, by UUID, as the server last listed it.
    #[serde(default)]
    ids: BTreeMap<String, String>,
    /// Old IDs and what they became, for telling the user.
    #[serde(default)]
    renamed: BTreeMap<String, String>,
}

/// What an upload asks the server to check first.
#[derive(Clone, Copy)]
enum Condition {
    /// Only if the document is still at this revision.
    Revision(i32),
    /// Only if the document doesn't exist.
    New,
    /// Overwrite whatever is there.
    None,
}

enum Upload {
    Done(i32),
    /// Someone else changed (or created, or deleted) the document first.
    Conflict,
    Offline,
}

/// The shared documents of one kind on the server cfcli is signed in to.
pub struct Shared<D> {
    credentials: Credentials,
    /// `synced/<server>` next to the cfcli config.
    dir: PathBuf,
    sync: bool,
    client: reqwest::Client,
    /// The server didn't answer once; don't wait for it again in this command.
    offline: AtomicBool,
    kind: PhantomData<fn() -> D>,
}

impl<D: Document> Shared<D> {
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
        move_old_swarm_copies(&dir);
        let client = reqwest::Client::builder()
            .user_agent(concat!("cfcli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Some(Shared { credentials, dir, sync, client, offline: AtomicBool::new(false), kind: PhantomData }))
    }

    pub fn host(&self) -> &str {
        auth::host(&self.credentials.server)
    }

    pub fn sync(&self) -> bool {
        self.sync
    }

    // ---- The copies and their state ----

    fn copies(&self, org: &str) -> Store<D> {
        Store::at(self.dir.join(D::FOLDER).join(org))
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn state(&self) -> Result<State> {
        read_state(&self.state_path())
    }

    /// The copies of this kind.
    fn copy_states(&self) -> Result<BTreeMap<String, CopyState>> {
        Ok(self.state()?.remove(D::FOLDER).unwrap_or_default())
    }

    fn save_state(&self, state: &State) -> Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.state_path();
        let tmp = self.dir.join(".state.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(state)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    pub fn copy_state(&self, id: &SharedId) -> Result<Option<CopyState>> {
        Ok(self.copy_states()?.get(&id.to_string()).copied())
    }

    fn set_copy_state(&self, id: &SharedId, copy: Option<CopyState>) -> Result<()> {
        let mut state = self.state()?;
        let copies = state.entry(D::FOLDER.to_string()).or_default();
        match copy {
            Some(copy) => copies.insert(id.to_string(), copy),
            None => copies.remove(&id.to_string()),
        };
        self.save_state(&state)
    }

    /// Keep `document` as the copy of `id`.
    fn keep(&self, id: &SharedId, document: &D, copy: CopyState) -> Result<()> {
        self.copies(&id.org).save(&id.name, document)?;
        self.set_copy_state(id, Some(copy))
    }

    fn forget(&self, id: &SharedId) -> Result<()> {
        let copies = self.copies(&id.org);
        if copies.exists(&id.name) {
            copies.delete(&id.name)?;
        }
        self.set_copy_state(id, None)
    }

    fn has_copy(&self, id: &SharedId) -> bool {
        self.copies(&id.org).exists(&id.name)
    }

    /// The copy, without asking the server. For shell completion and the
    /// like, which must never wait for the network.
    pub fn cached(&self, id: &SharedId) -> Result<D> {
        if !self.has_copy(id) {
            if let Some(now) = self.renamed(id)? {
                bail!(renamed_error::<D>(id, &now));
            }
            bail!(CliError::NotFound(format!("a copy of {} '{}' on this computer", D::NOUN, id)));
        }
        self.copies(&id.org).load(&id.name)
    }

    /// The shared documents there are copies of, without asking the server.
    pub fn cached_ids(&self) -> Vec<SharedId> {
        self.copy_states()
            .map(|copies| copies.keys().filter_map(|id| SharedId::parse(id).ok().flatten()).collect())
            .unwrap_or_default()
    }

    // ---- Organization IDs ----

    fn org_ids(&self) -> Result<OrgIds> {
        let path = self.dir.join("orgs.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OrgIds::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn save_org_ids(&self, orgs: &OrgIds) -> Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.dir.join("orgs.json");
        std::fs::write(&path, serde_json::to_string_pretty(orgs)?).with_context(|| format!("writing {}", path.display()))
    }

    /// What `id` is called now, if the user changed the ID of its
    /// organization. Also selects the swarm under the new ID, if the
    /// selected one is in that organization.
    fn renamed(&self, id: &SharedId) -> Result<Option<SharedId>> {
        let Some(org) = self.org_ids()?.renamed.get(&id.org).cloned() else {
            return Ok(None);
        };
        let now = SharedId { org, name: id.name.clone() };
        select_renamed(&[(id.org.clone(), now.org.clone())]);
        Ok(Some(now))
    }

    /// Follow the organizations whose ID the user changed since the last
    /// list: the copies of every kind move to the new ID, and so do the
    /// selected swarm and the swarms' links to lighthouse configurations.
    fn follow_renames(&self, server: &[ServerItem]) -> Result<()> {
        let mut orgs = self.org_ids()?;
        let before = serde_json::to_string(&orgs)?;
        let mut renames: Vec<(String, String)> = Vec::new();
        for item in server {
            let Some(uuid) = &item.org_uuid else { continue };
            if let Some(old) = orgs.ids.insert(uuid.clone(), item.id.org.clone()) {
                if old != item.id.org && !renames.contains(&(old.clone(), item.id.org.clone())) {
                    renames.push((old, item.id.org.clone()));
                }
            }
        }
        for (old, new) in &renames {
            for to in orgs.renamed.values_mut().filter(|to| *to == old) {
                *to = new.clone();
            }
            orgs.renamed.insert(old.clone(), new.clone());
        }
        // An ID in use is no longer an old one.
        for item in server {
            orgs.renamed.remove(&item.id.org);
        }
        if serde_json::to_string(&orgs)? != before {
            self.save_org_ids(&orgs)?;
        }
        if renames.is_empty() {
            return Ok(());
        }

        // Take all the moving copies out first, so that two organizations
        // can swap IDs. The copies of every kind move, not only this one's.
        let mut state = self.state()?;
        let mut moving = Vec::new();
        for (kind, copies) in state.iter_mut() {
            for (old, new) in &renames {
                let ids: Vec<SharedId> = copies
                    .keys()
                    .filter_map(|key| SharedId::parse(key).ok().flatten())
                    .filter(|id| id.org == *old)
                    .collect();
                for id in ids {
                    let copy = copies.remove(&id.to_string()).unwrap_or_default();
                    let path = self.dir.join(kind).join(old).join(format!("{}.yaml", id.name));
                    let yaml = match std::fs::read_to_string(&path) {
                        Ok(yaml) => {
                            std::fs::remove_file(&path).with_context(|| format!("moving {}", path.display()))?;
                            Some(yaml)
                        }
                        Err(_) => None,
                    };
                    moving.push((kind.clone(), SharedId { org: new.clone(), name: id.name }, copy, yaml));
                }
            }
        }
        for (kind, id, copy, yaml) in moving {
            if let Some(yaml) = yaml {
                let dir = self.dir.join(&kind).join(&id.org);
                std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
                let path = dir.join(format!("{}.yaml", id.name));
                std::fs::write(&path, yaml).with_context(|| format!("writing {}", path.display()))?;
            }
            state.entry(kind).or_default().insert(id.to_string(), copy);
        }
        self.save_state(&state)?;
        for kind in state.keys() {
            for (old, _) in &renames {
                // Only goes when empty.
                let _ = std::fs::remove_dir(self.dir.join(kind).join(old));
            }
        }

        // Swarms name their lighthouse configuration with the organization's
        // ID: the local ones and the copies. (The server sends the copies
        // with the new ID anyway; this keeps those not synced yet right.)
        let mut stores = vec![Store::<Swarm>::open()?];
        for key in state.get(Swarm::FOLDER).into_iter().flat_map(|copies| copies.keys()) {
            if let Some(id) = SharedId::parse(key).ok().flatten() {
                let store = Store::<Swarm>::at(self.dir.join(Swarm::FOLDER).join(&id.org));
                if !stores.iter().any(|s| s.dir() == store.dir()) {
                    stores.push(store);
                }
            }
        }
        for store in &stores {
            for id in store.ids()? {
                let Ok(mut swarm) = store.load(&id) else { continue };
                let mut changed = false;
                for (old, new) in &renames {
                    changed |= swarm.rename_org(old, new);
                }
                if changed {
                    store.save(&id, &swarm)?;
                }
            }
        }

        select_renamed(&renames);
        for (old, new) in &renames {
            println!(
                "Your organization '{}' on {} is now called '{}': its swarms and lighthouse configs are '{}/<id>'",
                old,
                self.host(),
                new,
                new
            );
        }
        Ok(())
    }

    // ---- Talking to the server ----

    fn url(&self, id: &SharedId) -> Result<reqwest::Url> {
        auth::endpoint(&self.credentials.server, &format!("/api/v1/{}/{}/{}", D::FOLDER, id.org, id.name))
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
                        "Can't reach {}: using the copies of shared {}s on this computer. \
                         Changes are kept and uploaded {}.",
                        self.host(),
                        D::NOUN,
                        if self.sync {
                            "when it answers again".to_string()
                        } else {
                            format!("with 'cfcli {} push'", D::COMMAND)
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

    /// The documents on the server, in all the user's organizations.
    async fn server_list(&self) -> Result<Option<Vec<ServerItem>>> {
        let url = auth::endpoint(&self.credentials.server, &format!("/api/v1/{}", D::FOLDER))?;
        let Some(response) = self.send(self.client.get(url)).await? else {
            return Ok(None);
        };
        let items: Vec<serde_json::Map<String, serde_json::Value>> =
            auth::answer(&self.credentials.server, response).await?;
        let mut listed = Vec::new();
        for item in items {
            let text = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let Some(id) = SharedId::parse(&text("id")).ok().flatten() else {
                continue;
            };
            let number = |key: &str| item.get(key).and_then(|v| v.as_u64()).unwrap_or_default();
            listed.push(ServerItem {
                id,
                org_uuid: Some(text("org_uuid")).filter(|uuid| !uuid.is_empty()),
                name: text("name"),
                count: number(D::SERVER_COUNT) as usize,
                revision: number("revision") as i32,
            });
        }
        self.follow_renames(&listed)?;
        Ok(Some(listed))
    }

    /// A list the server keeps about `id`, `GET /api/v1/<kind>/<org>/<id>/<what>`
    /// (the swarms that name a lighthouse configuration). None when the
    /// server can't be reached; empty when it doesn't have `id`.
    pub async fn related(&self, id: &SharedId, what: &str) -> Result<Option<Vec<serde_json::Map<String, serde_json::Value>>>> {
        let url = auth::endpoint(
            &self.credentials.server,
            &format!("/api/v1/{}/{}/{}/{}", D::FOLDER, id.org, id.name, what),
        )?;
        let Some(response) = self.send_quietly(self.client.get(url), true).await? else {
            return Ok(None);
        };
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Some(Vec::new()));
        }
        auth::answer(&self.credentials.server, response).await.map(Some)
    }

    /// After the server answered 404 for `id`: fail saying what it is called
    /// now, if the user changed the ID of its organization. Gets the list,
    /// which moves the copies.
    async fn fail_if_renamed(&self, id: &SharedId) -> Result<()> {
        self.server_list().await?;
        match self.renamed(id)? {
            Some(now) => bail!(renamed_error::<D>(id, &now)),
            None => Ok(()),
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
                self.fail_if_renamed(id).await?;
                // Deleted on the server, or no longer visible to the user.
                if copy.pending {
                    bail!(CliError::NotFound(format!(
                        "{} '{}' on {}, though this computer has changes to it that aren't uploaded; \
                         'cfcli {} push --force {}' creates it again with them",
                        D::NOUN,
                        id,
                        self.host(),
                        D::COMMAND,
                        id
                    )));
                }
                self.forget(id)?;
                bail!(CliError::NotFound(format!("{} '{}' on {}", D::NOUN, id, self.host())))
            }
            status if status.is_success() => {
                let revision = etag_revision(&response)?;
                let yaml = response.text().await?;
                let document = D::from_yaml(&yaml).with_context(|| format!("{} '{}' from {}", D::NOUN, id, self.host()))?;
                self.keep(id, &document, CopyState { revision, pending: false })?;
                Ok(true)
            }
            _ => auth::answer::<serde_json::Value>(&self.credentials.server, response).await.map(|_| true),
        }
    }

    async fn upload(&self, id: &SharedId, document: &D, condition: Condition) -> Result<Upload> {
        let mut request = self
            .client
            .put(self.url(id)?)
            .header(header::CONTENT_TYPE, "application/yaml")
            .body(document.to_yaml()?);
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
        if response.status() == StatusCode::NOT_FOUND {
            self.fail_if_renamed(id).await?;
        }
        #[derive(Deserialize)]
        struct Uploaded {
            revision: i32,
        }
        let uploaded: Uploaded = auth::answer(&self.credentials.server, response).await?;
        Ok(Upload::Done(uploaded.revision))
    }

    // ---- What the commands do ----

    /// Upload the copy of `id` if it has changes. Fails on a conflict, saying
    /// how to settle it. Returns false when the server can't be reached.
    async fn push_pending(&self, id: &SharedId, force: bool) -> Result<bool> {
        let Some(copy) = self.copy_state(id)? else {
            return Ok(true);
        };
        if !copy.pending {
            return Ok(true);
        }
        let document = self.copies(&id.org).load(&id.name)?;
        let condition = match (force, copy.revision) {
            (true, _) => Condition::None,
            (false, 0) => Condition::New,
            (false, revision) => Condition::Revision(revision),
        };
        match self.upload(id, &document, condition).await? {
            Upload::Done(revision) => {
                self.set_copy_state(id, Some(CopyState { revision, pending: false }))?;
                Ok(true)
            }
            Upload::Offline => Ok(false),
            Upload::Conflict => bail!(CliError::InvalidValue(format!(
                "{} '{}' changed on {} since this computer's copy ({}). Keep theirs with \
                 'cfcli {} pull --force {}', or yours with 'cfcli {} push --force {}'",
                D::NOUN,
                id,
                self.host(),
                match copy.revision {
                    0 => "someone created it first".to_string(),
                    revision => format!("revision {}", revision),
                },
                D::COMMAND,
                id,
                D::COMMAND,
                id
            ))),
        }
    }

    /// The document, checked with the server first when sync is on (or when
    /// there's no copy yet).
    pub async fn load(&self, id: &SharedId) -> Result<D> {
        if (self.sync || !self.has_copy(id)) && self.push_pending(id, false).await? {
            self.download(id, false).await?;
        }
        self.cached(id)
    }

    /// Change the document. `change` may run more than once: when someone
    /// else uploaded in between, it runs again on their version. With sync
    /// off, or without the server, it changes the copy and keeps it for a
    /// push.
    pub async fn change<T>(&self, id: &SharedId, mut change: impl FnMut(&mut D) -> Result<T>) -> Result<T> {
        let mut online = false;
        if self.sync || !self.has_copy(id) {
            online = self.push_pending(id, false).await? && self.download(id, false).await?;
        }
        if online {
            for _ in 0..3 {
                let mut document = self.cached(id)?;
                let before = document.to_yaml()?;
                let revision = self.copy_state(id)?.unwrap_or_default().revision;
                let result = change(&mut document)?;
                if document.to_yaml()? == before {
                    return Ok(result);
                }
                match self.upload(id, &document, Condition::Revision(revision)).await? {
                    Upload::Done(revision) => {
                        self.keep(id, &document, CopyState { revision, pending: false })?;
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
        let mut document = self.cached(id)?;
        let before = document.to_yaml()?;
        let result = change(&mut document)?;
        if document.to_yaml()? == before {
            return Ok(result);
        }
        let revision = self.copy_state(id)?.unwrap_or_default().revision;
        self.keep(id, &document, CopyState { revision, pending: true })?;
        if !self.sync {
            println!("Changed this computer's copy of '{}'; 'cfcli {} push' uploads it", id, D::COMMAND);
        }
        Ok(result)
    }

    /// Create the document on the server. Returns false when it already
    /// exists. Without the server, the copy waits for a push.
    pub async fn create(&self, id: &SharedId, document: &D) -> Result<bool> {
        if self.has_copy(id) && self.copy_state(id)?.is_some_and(|c| c.pending) {
            return Ok(false);
        }
        match self.upload(id, document, Condition::New).await? {
            Upload::Done(revision) => self.keep(id, document, CopyState { revision, pending: false })?,
            Upload::Conflict => return Ok(false),
            Upload::Offline => {
                self.keep(id, document, CopyState { revision: 0, pending: true })?;
                println!("Created '{}' on this computer; 'cfcli {} push' uploads it", id, D::COMMAND);
            }
        }
        Ok(true)
    }

    /// Delete the document on the server, and the copy. Needs the server,
    /// unless the document never got there.
    pub async fn delete(&self, id: &SharedId) -> Result<()> {
        let copy = self.copy_state(id)?;
        if copy.is_some_and(|c| c.revision == 0) {
            return self.forget(id);
        }
        let mut request = self.client.delete(self.url(id)?);
        // With sync off, the user decided on the copy they have: don't delete
        // what someone uploaded since. With sync on, the copy is just the
        // last one used, so the document goes whatever its revision.
        if let Some(copy) = copy.filter(|_| !self.sync) {
            request = request.header(header::IF_MATCH, format!("\"{}\"", copy.revision));
        }
        let response = self.send_online(request, &format!("deleting a shared {}", D::NOUN)).await?;
        match response.status() {
            status if status.is_success() => self.forget(id),
            StatusCode::NOT_FOUND => {
                self.fail_if_renamed(id).await?;
                self.forget(id)?;
                bail!(CliError::NotFound(format!("{} '{}' on {}", D::NOUN, id, self.host())))
            }
            StatusCode::PRECONDITION_FAILED => bail!(CliError::InvalidValue(format!(
                "{} '{}' changed on {} since this computer's copy; \
                 'cfcli {} pull --force {}' gets the new version",
                D::NOUN,
                id,
                self.host(),
                D::COMMAND,
                id
            ))),
            _ => auth::answer::<serde_json::Value>(&self.credentials.server, response).await.map(|_| ()),
        }
    }

    /// The shared documents: from the server when sync is on (and it
    /// answers), else from the copies. Documents created here and not
    /// uploaded are included either way.
    pub async fn list(&self) -> Result<Vec<Listed>> {
        let server = if self.sync { self.server_list().await? } else { None };
        // After the list, which moves the copies of renamed organizations.
        let copies = self.copy_states()?;
        let mut listed = Vec::new();
        match server {
            Some(server) => {
                for item in server {
                    // The server's revision, which the next command gets,
                    // unless the copy has changes to upload.
                    let copy = match copies.get(&item.id.to_string()) {
                        Some(copy) if copy.pending => *copy,
                        _ => CopyState { revision: item.revision, pending: false },
                    };
                    listed.push(Listed { id: item.id, name: item.name, count: item.count, copy });
                }
                // Copies of documents gone from the server go too, unless
                // they have changes to upload.
                for (key, copy) in &copies {
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
                for (key, copy) in &copies {
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
        let (name, count) = match self.cached(&id) {
            Ok(document) => document.summary(),
            Err(e) => (format!("(can't read: {:#})", e), 0),
        };
        Listed { id, name, count, copy }
    }

    /// Get the latest version of every shared document (or of one). Copies
    /// with changes that aren't uploaded are kept, unless `force`.
    pub async fn pull(&self, only: Option<&SharedId>, force: bool) -> Result<()> {
        let Some(server) = self.server_list().await? else {
            bail!(CliError::Connection(format!("{} can't be reached", self.host())));
        };
        let on_server: Vec<SharedId> = server.into_iter().map(|item| item.id).collect();
        if let Some(only) = only {
            if let Some(now) = self.renamed(only)? {
                bail!(renamed_error::<D>(only, &now));
            }
            if !on_server.contains(only) {
                bail!(CliError::NotFound(format!("{} '{}' on {}", D::NOUN, only, self.host())));
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
        // Copies of documents deleted on the server.
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
            println!(
                "Upload kept changes with 'cfcli {} push', or drop them with 'pull --force'",
                D::COMMAND
            );
        }
        Ok(())
    }

    /// Upload the changes made while sync was off or the server was out of
    /// reach. `force` overwrites what others uploaded in between.
    pub async fn push(&self, only: Option<&SharedId>, force: bool) -> Result<()> {
        // Copies of organizations with a new ID move to it first.
        self.server_list().await?;
        if let Some(only) = only {
            if let Some(now) = self.renamed(only)? {
                bail!(renamed_error::<D>(only, &now));
            }
        }
        let pending: Vec<SharedId> = self
            .copy_states()?
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

fn renamed_error<D: Document>(id: &SharedId, now: &SharedId) -> CliError {
    CliError::NotFound(format!(
        "{} '{}' (your organization '{}' is now called '{}'; use '{}')",
        D::NOUN,
        id,
        id.org,
        now.org,
        now
    ))
}

/// Select the swarm under its organization's new ID, if one of `renames`
/// (old, new) changed the selected swarm's.
fn select_renamed(renames: &[(String, String)]) {
    let Ok(mut config) = confy::load::<Config>("cf-cli", None) else {
        return;
    };
    let Some(selected) = config.swarm.as_deref().and_then(|id| SharedId::parse(id).ok().flatten()) else {
        return;
    };
    if let Some((_, new)) = renames.iter().find(|(old, _)| *old == selected.org) {
        let now = SharedId { org: new.clone(), name: selected.name };
        config.swarm = Some(now.to_string());
        if confy::store("cf-cli", None, config).is_ok() {
            println!("The selected swarm is '{}' now", now);
        }
    }
}

fn read_state(path: &Path) -> Result<State> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// cfcli kept the copies of shared swarms in `synced/<server>/<org>/`
/// before there were other kinds of documents; move them to
/// `synced/<server>/swarms/<org>/`.
fn move_old_swarm_copies(dir: &Path) {
    let swarms = dir.join("swarms");
    if swarms.exists() {
        return;
    }
    let Ok(mut state) = read_state(&dir.join("state.json")) else {
        return;
    };
    let orgs: std::collections::BTreeSet<String> = state
        .remove("swarms")
        .unwrap_or_default()
        .keys()
        .filter_map(|id| id.split_once('/').map(|(org, _)| org.to_string()))
        .collect();
    for org in orgs {
        let old = dir.join(&org);
        if old.is_dir() && std::fs::create_dir_all(&swarms).is_ok() {
            let _ = std::fs::rename(&old, swarms.join(&org));
        }
    }
}

/// The revision in a response's ETag, `"N"`.
fn etag_revision(response: &Response) -> Result<i32> {
    response
        .headers()
        .get(header::ETAG)
        .and_then(|etag| etag.to_str().ok())
        .and_then(|etag| etag.trim_start_matches("W/").trim_matches('"').parse().ok())
        .context("the server sent a document without its revision")
}

// ---- Both behind one kind of ID ----

/// Local and shared documents of one kind behind one kind of ID: `<id>` is
/// a file in the local folder, `<org>/<id>` a document on the server.
pub struct Documents<D> {
    pub local: Store<D>,
    /// None when cfcli isn't signed in.
    pub shared: Option<Shared<D>>,
}

impl<D: Document> Documents<D> {
    pub fn open(config: &Config) -> Result<Self> {
        Ok(Documents { local: Store::open()?, shared: Shared::open(config.sync_on())? })
    }

    /// For shell completion: only [`Documents::cached`] and
    /// [`Documents::cached_ids`] are used, which never ask the server.
    pub fn open_cached() -> Result<Self> {
        Ok(Documents { local: Store::open()?, shared: Shared::open(false)? })
    }

    pub fn shared(&self, id: &SharedId) -> Result<&Shared<D>> {
        self.shared.as_ref().ok_or_else(|| {
            CliError::NotFound(format!(
                "a sign-in: '{}' is a shared {}, sign in with 'cfcli auth login'",
                id,
                D::NOUN
            ))
            .into()
        })
    }

    pub async fn load(&self, id: &str) -> Result<D> {
        match SharedId::parse(id)? {
            None => self.local.load(id),
            Some(shared) => self.shared(&shared)?.load(&shared).await,
        }
    }

    /// The document without asking the server (a shared one's copy), for
    /// shell completion and pickers.
    pub fn cached(&self, id: &str) -> Result<D> {
        match SharedId::parse(id)? {
            None => self.local.load(id),
            Some(shared) => self.shared(&shared)?.cached(&shared),
        }
    }

    /// Change a document. `change` may run more than once for a shared one
    /// (see [`Shared::change`]), so it must not ask the user anything.
    pub async fn change<T>(&self, id: &str, mut change: impl FnMut(&mut D) -> Result<T>) -> Result<T> {
        match SharedId::parse(id)? {
            None => {
                let mut document = self.local.load(id)?;
                let before = document.to_yaml()?;
                let result = change(&mut document)?;
                if document.to_yaml()? != before {
                    self.local.save(id, &document)?;
                }
                Ok(result)
            }
            Some(shared) => self.shared(&shared)?.change(&shared, change).await,
        }
    }

    /// Create a document. A shared one is created on the server.
    pub async fn create(&self, id: &str, document: &D) -> Result<()> {
        match SharedId::parse(id)? {
            None => {
                check_id::<D>(id)?;
                if self.local.exists(id) {
                    bail!(CliError::InvalidValue(format!("{} '{}' already exists", D::NOUN, id)));
                }
                self.local.save(id, document)
            }
            Some(shared_id) => {
                let shared = self.shared(&shared_id)?;
                if !shared.create(&shared_id, document).await? {
                    bail!(CliError::InvalidValue(format!(
                        "{} '{}' already exists on {}",
                        D::NOUN,
                        id,
                        shared.host()
                    )));
                }
                Ok(())
            }
        }
    }

    /// Delete a document. A shared one is deleted on the server, for everyone.
    pub async fn delete(&self, id: &str) -> Result<()> {
        match SharedId::parse(id)? {
            None => self.local.delete(id),
            Some(shared) => self.shared(&shared)?.delete(&shared).await,
        }
    }

    /// Local documents and the shared ones there are copies of, without
    /// asking the server.
    pub fn cached_ids(&self) -> Result<Vec<String>> {
        let mut ids = self.local.ids()?;
        if let Some(shared) = &self.shared {
            ids.extend(shared.cached_ids().into_iter().map(|id| id.to_string()));
        }
        Ok(ids)
    }

    /// Where a document is kept, for the user.
    pub fn place(&self, id: &str) -> String {
        match (&self.shared, SharedId::parse(id)) {
            (Some(shared), Ok(Some(_))) => shared.host().to_string(),
            _ => "this computer".to_string(),
        }
    }

    /// The server for `pull`/`push`, and the one shared document to sync if
    /// given.
    pub fn for_sync(&self, id: Option<&str>) -> Result<(&Shared<D>, Option<SharedId>)> {
        let Some(shared) = &self.shared else {
            bail!(CliError::NotFound(format!(
                "a sign-in: shared {}s need 'cfcli auth login'",
                D::NOUN
            )));
        };
        let only = match id {
            None => None,
            Some(id) => Some(SharedId::parse(id)?.ok_or_else(|| {
                CliError::InvalidValue(format!(
                    "'{}' is a local {}; only shared ones (<org>/<id>) are pulled and pushed",
                    id,
                    D::NOUN
                ))
            })?),
        };
        Ok((shared, only))
    }
}

/// A document as `list` and the pickers show it.
pub struct Entry {
    pub id: String,
    pub name: String,
    /// The item count, "?" when the file can't be read.
    pub count: String,
    /// Where it is kept: "this computer" or the server.
    pub place: String,
    pub revision: Option<i32>,
    pub pending: bool,
}

impl Entry {
    pub fn stored(&self) -> String {
        match self.revision {
            None => self.place.clone(),
            Some(0) => format!("{}, not uploaded yet", self.place),
            Some(revision) if self.pending => format!("{}, revision {}, changes not pushed", self.place, revision),
            Some(revision) => format!("{}, revision {}", self.place, revision),
        }
    }
}

impl<D: Document> Documents<D> {
    /// The local documents, then the shared ones.
    pub async fn entries(&self) -> Result<Vec<Entry>> {
        let mut entries: Vec<Entry> = self
            .local
            .ids()?
            .into_iter()
            .map(|id| {
                let (name, count) = match self.local.load(&id) {
                    Ok(document) => {
                        let (name, count) = document.summary();
                        (name, count.to_string())
                    }
                    Err(e) => (format!("(can't read: {:#})", e), "?".to_string()),
                };
                Entry { id, name, count, place: "this computer".to_string(), revision: None, pending: false }
            })
            .collect();
        if let Some(shared) = &self.shared {
            for listed in shared.list().await? {
                entries.push(Entry {
                    id: listed.id.to_string(),
                    name: listed.name,
                    count: listed.count.to_string(),
                    place: shared.host().to_string(),
                    revision: Some(listed.copy.revision),
                    pending: listed.copy.pending,
                });
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shared_and_local_ids() {
        assert_eq!(SharedId::parse("lab").unwrap(), None);
        let id = SharedId::parse("bitcraze-lab/cage").unwrap().unwrap();
        assert_eq!((id.org.as_str(), id.name.as_str()), ("bitcraze-lab", "cage"));
        assert_eq!(id.to_string(), "bitcraze-lab/cage");
        for bad in ["Lab/cage", "/cage", "lab/", "lab/a/b", "lab/.x", "la b/x"] {
            assert!(SharedId::parse(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn moves_old_swarm_copies() {
        let dir = std::env::temp_dir().join(format!("cfcli-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lab")).unwrap();
        std::fs::write(dir.join("lab").join("cage.yaml"), "name: x\nunits: []\n").unwrap();
        std::fs::write(dir.join("state.json"), r#"{"swarms": {"lab/cage": {"revision": 3, "pending": true}}}"#).unwrap();
        move_old_swarm_copies(&dir);
        assert!(dir.join("swarms").join("lab").join("cage.yaml").is_file());
        assert!(!dir.join("lab").exists());
        // The state file has the same shape as before.
        let state = read_state(&dir.join("state.json")).unwrap();
        assert_eq!(state["swarms"]["lab/cage"].revision, 3);
        // Only once.
        std::fs::create_dir_all(dir.join("lab")).unwrap();
        move_old_swarm_copies(&dir);
        assert!(dir.join("lab").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
