//! Swarms stored on disk, in the format Swarmkeeper uses.
//!
//! Each swarm is one YAML file, `<id>.yaml`, in a `swarms` folder next to
//! the cfcli config file. The ID is the file name; the `name` inside the file
//! is only shown to the user. The format:
//!
//! ```yaml
//! name: Set1 (100 units)
//! description: Swarm of 100 Crazyflies on channel 72
//! units:
//!   - uri: radio:///72/2M/D91F700101
//!     name: CF-101
//!     description: Crazyflie 101
//! ```
//!
//! Fields cfcli doesn't know are kept when a file is rewritten, so nothing
//! Swarmkeeper adds later is lost. A swarm may name the lighthouse
//! configuration it flies in (`lighthouse: lab/cage`).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::modules::documents::{self, Document};
use crate::utils::radio::{self, RadioUri};

/// One swarm file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Swarm {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The lighthouse configuration the swarm flies in: a local `<id>` or a
    /// shared `<org>/<id>` (see `cfcli lh config`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighthouse: Option<String>,
    // Always written, even when empty: Swarmkeeper requires the field.
    #[serde(default)]
    pub units: Vec<Unit>,
    #[serde(flatten)]
    pub extra: serde_yaml::Mapping,
}

/// One Crazyflie in a swarm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub uri: String,
    /// The short name shown in the output and used to pick Crazyflies.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: serde_yaml::Mapping,
}

/// What makes two URIs the same Crazyflie: the radio index doesn't count,
/// so `radio://0/80/2M/E7E7E7E7E7` and `radio:///80/2M/e7e7e7e7e7` match.
#[derive(Debug, PartialEq, Eq)]
enum LinkKey {
    Radio { channel: u8, address: String },
    Other(String),
}

fn link_key(uri: &str) -> Result<LinkKey> {
    Ok(match RadioUri::parse(uri)? {
        Some(radio_uri) => LinkKey::Radio {
            channel: radio_uri.channel,
            address: radio_uri.address,
        },
        None => LinkKey::Other(uri.to_string()),
    })
}

/// Check a URI that is about to go into a swarm.
pub fn check_uri(uri: &str) -> Result<()> {
    if RadioUri::parse(uri)?.is_none() && !uri.starts_with("usb://") {
        bail!(CliError::InvalidValue(format!(
            "URI '{}': only radio:// and usb:// URIs can be added to a swarm",
            uri
        )));
    }
    Ok(())
}

/// Check a short name: commas would break `--cf a,b` lists.
pub fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!(CliError::InvalidValue("a Crazyflie name can't be empty".to_string()));
    }
    if name.contains(',') {
        bail!(CliError::InvalidValue(format!("Crazyflie name '{}' contains a comma", name)));
    }
    Ok(())
}

/// Check a swarm ID, which is also a file name.
pub fn check_id(id: &str) -> Result<()> {
    documents::check_id::<Swarm>(id)
}

impl Swarm {
    pub fn new(name: String, description: Option<String>) -> Self {
        Swarm { name, description, lighthouse: None, units: Vec::new(), extra: serde_yaml::Mapping::new() }
    }

    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let swarm: Swarm = serde_yaml::from_str(yaml)?;
        swarm.check()?;
        Ok(swarm)
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(serde_yaml::to_string(self)?)
    }

    /// Check that every URI is valid and every name is usable and unique.
    pub fn check(&self) -> Result<()> {
        for (i, unit) in self.units.iter().enumerate() {
            check_uri(&unit.uri)?;
            check_name(&unit.name)?;
            if let Some(other) = self.units[..i].iter().find(|u| u.name.eq_ignore_ascii_case(&unit.name)) {
                bail!(CliError::InvalidValue(format!(
                    "two Crazyflies are named '{}' ({} and {})",
                    unit.name, other.uri, unit.uri
                )));
            }
        }
        Ok(())
    }

    /// Empty the radio of every `radio://0/` URI. Returns how many changed.
    pub fn use_any_radio(&mut self) -> usize {
        let mut changed = 0;
        for unit in &mut self.units {
            let (uri, did_change) = radio::any_radio(&unit.uri);
            if did_change {
                unit.uri = uri;
                changed += 1;
            }
        }
        changed
    }

    /// Find a Crazyflie by name (ignoring case) or by URI.
    pub fn find(&self, query: &str) -> Option<usize> {
        if let Some(i) = self.units.iter().position(|u| u.name.eq_ignore_ascii_case(query)) {
            return Some(i);
        }
        let key = link_key(query).ok()?;
        self.units.iter().position(|u| link_key(&u.uri).ok().as_ref() == Some(&key))
    }

    /// Like [`Swarm::find`], but a miss is an error naming the swarm.
    pub fn find_or_err(&self, id: &str, query: &str) -> Result<usize> {
        self.find(query)
            .ok_or_else(|| CliError::NotFound(format!("Crazyflie '{}' in swarm '{}'", query, id)).into())
    }

    /// The Crazyflie already in the swarm on the same link as `uri`, if any.
    pub fn find_link(&self, uri: &str) -> Result<Option<usize>> {
        let key = link_key(uri)?;
        Ok(self.units.iter().position(|u| link_key(&u.uri).ok().as_ref() == Some(&key)))
    }

    /// The first `CF-NN` name nobody uses.
    pub fn next_name(&self) -> String {
        (1..)
            .map(|n| format!("CF-{:02}", n))
            .find(|name| !self.units.iter().any(|u| u.name.eq_ignore_ascii_case(name)))
            .expect("there is always a free name")
    }

    /// The Crazyflies a command acts on, as indices in file order: all of
    /// them, only those in `only` if given, minus those in `exclude`.
    pub fn select(&self, id: &str, only: &[String], exclude: &[String]) -> Result<Vec<usize>> {
        if self.units.is_empty() {
            bail!(CliError::NotFound(format!(
                "Crazyflies in swarm '{}' (add some with 'cfcli swarm config add')",
                id
            )));
        }
        let mut selected: Vec<usize> = if only.is_empty() {
            (0..self.units.len()).collect()
        } else {
            let mut chosen = only.iter().map(|q| self.find_or_err(id, q)).collect::<Result<Vec<_>>>()?;
            chosen.sort_unstable();
            chosen.dedup();
            chosen
        };
        for query in exclude {
            let i = self.find_or_err(id, query)?;
            selected.retain(|s| *s != i);
        }
        if selected.is_empty() {
            bail!(CliError::InvalidValue("--cf/--exclude leave no Crazyflies to act on".to_string()));
        }
        Ok(selected)
    }
}

/// The folder holding the swarm files, next to the cfcli config file.
pub type Store = documents::Store<Swarm>;

impl Document for Swarm {
    const FOLDER: &'static str = "swarms";
    const NOUN: &'static str = "swarm";
    const COMMAND: &'static str = "swarm config";
    const SERVER_COUNT: &'static str = "units";

    fn from_yaml(yaml: &str) -> Result<Self> {
        Swarm::from_yaml(yaml)
    }

    fn to_yaml(&self) -> Result<String> {
        Swarm::to_yaml(self)
    }

    fn summary(&self) -> (String, usize) {
        (self.name.clone(), self.units.len())
    }

    /// The organization of a shared lighthouse configuration.
    fn rename_org(&mut self, old: &str, new: &str) -> bool {
        let Some((org, config)) = self.lighthouse.as_deref().and_then(|l| l.split_once('/')) else {
            return false;
        };
        if org != old {
            return false;
        }
        self.lighthouse = Some(format!("{}/{}", new, config));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWARMKEEPER: &str = "\
name: 49-unit swarm
description: Swarm of 49 Crazyflies on channel 84
units:
  - uri: radio://0/84/2M/D91F700101
    name: CF-01
    description: Crazyflie 01
  - uri: radio://0/84/2M/D91F70010A
    name: CF with LPS + LH
    description: Crazyflie 10
";

    fn unit(uri: &str, name: &str) -> Unit {
        Unit { uri: uri.to_string(), name: name.to_string(), description: None, extra: serde_yaml::Mapping::new() }
    }

    fn swarm(units: &[(&str, &str)]) -> Swarm {
        let mut swarm = Swarm::new("test".to_string(), None);
        swarm.units = units.iter().map(|(uri, name)| unit(uri, name)).collect();
        swarm
    }

    fn temp_store(test: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("cfcli-swarm-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::at(dir)
    }

    #[test]
    fn reads_a_swarmkeeper_file() {
        let swarm = Swarm::from_yaml(SWARMKEEPER).unwrap();
        assert_eq!(swarm.name, "49-unit swarm");
        assert_eq!(swarm.units.len(), 2);
        assert_eq!(swarm.units[1].name, "CF with LPS + LH");
        assert_eq!(swarm.units[1].description.as_deref(), Some("Crazyflie 10"));
    }

    #[test]
    fn writes_what_swarmkeeper_reads() {
        let mut swarm = Swarm::from_yaml(SWARMKEEPER).unwrap();
        swarm.use_any_radio();
        let yaml = swarm.to_yaml().unwrap();
        assert!(yaml.starts_with("name: 49-unit swarm\n"), "{}", yaml);
        assert!(yaml.contains("- uri: radio:///84/2M/D91F700101\n  name: CF-01\n"), "{}", yaml);
        assert_eq!(Swarm::from_yaml(&yaml).unwrap().units.len(), 2);
    }

    #[test]
    fn an_empty_swarm_still_has_units() {
        let yaml = Swarm::new("empty".to_string(), None).to_yaml().unwrap();
        assert_eq!(yaml, "name: empty\nunits: []\n");
    }

    #[test]
    fn keeps_fields_it_does_not_know() {
        let yaml = "name: s\nfuture: 1\nunits:\n- uri: radio://0/80/2M/E7E7E7E7E7\n  name: CF-01\n  color: red\n";
        let written = Swarm::from_yaml(yaml).unwrap().to_yaml().unwrap();
        assert!(written.contains("future: 1"), "{}", written);
        assert!(written.contains("color: red"), "{}", written);
    }

    #[test]
    fn rejects_duplicate_names_ignoring_case() {
        let yaml = "name: s\nunits:\n- uri: radio://0/80/2M/E7E7E7E701\n  name: CF-01\n- uri: radio://0/80/2M/E7E7E7E702\n  name: cf-01\n";
        let err = Swarm::from_yaml(yaml).unwrap_err();
        assert!(format!("{:#}", err).contains("two Crazyflies are named"), "{:#}", err);
    }

    #[test]
    fn rejects_names_with_commas_and_bad_uris() {
        assert!(Swarm::from_yaml("name: s\nunits:\n- uri: radio://0/80/2M/E7\n  name: a,b\n").is_err());
        assert!(Swarm::from_yaml("name: s\nunits:\n- uri: tcp://x\n  name: a\n").is_err());
        assert!(Swarm::from_yaml("name: s\nunits:\n- uri: radio://0/200/2M/E7\n  name: a\n").is_err());
    }

    #[test]
    fn use_any_radio_counts_the_changes() {
        let mut swarm = swarm(&[
            ("radio://0/80/2M/E7E7E7E701", "a"),
            ("radio://1/80/2M/E7E7E7E702", "b"),
            ("radio:///80/2M/E7E7E7E703", "c"),
        ]);
        assert_eq!(swarm.use_any_radio(), 1);
        assert_eq!(swarm.units[0].uri, "radio:///80/2M/E7E7E7E701");
        assert_eq!(swarm.units[1].uri, "radio://1/80/2M/E7E7E7E702");
        assert_eq!(swarm.units[2].uri, "radio:///80/2M/E7E7E7E703");
    }

    #[test]
    fn finds_by_name_or_uri() {
        let swarm = swarm(&[("radio:///80/2M/E7E7E7E701", "CF-01"), ("usb://0", "CF-02")]);
        assert_eq!(swarm.find("cf-01"), Some(0));
        assert_eq!(swarm.find("radio://0/80/2M/e7e7e7e701"), Some(0));
        assert_eq!(swarm.find("radio://1/80/2M/E7E7E7E701"), Some(0));
        assert_eq!(swarm.find("usb://0"), Some(1));
        assert_eq!(swarm.find("CF-03"), None);
        assert_eq!(swarm.find("radio://0/81/2M/E7E7E7E701"), None);
    }

    #[test]
    fn next_name_fills_the_first_gap() {
        assert_eq!(swarm(&[]).next_name(), "CF-01");
        let swarm = swarm(&[("radio:///80/2M/01", "CF-01"), ("radio:///80/2M/03", "cf-02"), ("radio:///80/2M/04", "CF-04")]);
        assert_eq!(swarm.next_name(), "CF-03");
    }

    #[test]
    fn select_applies_only_and_exclude_in_file_order() {
        let swarm = swarm(&[("radio:///80/2M/01", "CF-01"), ("radio:///80/2M/02", "CF-02"), ("radio:///80/2M/03", "CF-03")]);
        assert_eq!(swarm.select("s", &[], &[]).unwrap(), vec![0, 1, 2]);
        let only = vec!["CF-03".to_string(), "cf-01".to_string()];
        assert_eq!(swarm.select("s", &only, &[]).unwrap(), vec![0, 2]);
        assert_eq!(swarm.select("s", &[], &["CF-02".to_string()]).unwrap(), vec![0, 2]);
        assert!(swarm.select("s", &["CF-09".to_string()], &[]).is_err());
        assert!(swarm.select("s", &["CF-01".to_string()], &["CF-01".to_string()]).is_err());
    }

    #[test]
    fn checks_ids() {
        for id in ["set-1", "home", "a_b.c", "49"] {
            assert!(check_id(id).is_ok(), "{}", id);
        }
        for id in ["", ".hidden", "a/b", "a b", "../x"] {
            assert!(check_id(id).is_err(), "{}", id);
        }
    }

    #[test]
    fn store_round_trip() {
        let store = temp_store("round-trip");
        assert!(store.ids().unwrap().is_empty());

        store.save("b", &Swarm::new("B".to_string(), None)).unwrap();
        store.save("a", &Swarm::from_yaml(SWARMKEEPER).unwrap()).unwrap();
        assert_eq!(store.ids().unwrap(), vec!["a", "b"]);
        assert_eq!(store.load("a").unwrap().units.len(), 2);
        assert!(store.exists("b"));

        store.delete("b").unwrap();
        assert_eq!(store.ids().unwrap(), vec!["a"]);
        assert!(store.delete("b").is_err());
        assert!(store.load("b").is_err());

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
