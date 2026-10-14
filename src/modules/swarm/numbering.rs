//! Numbering new Crazyflies: a base name (`CF-`) and a base address
//! (`ABAD1DEA`), each followed by a number.
//!
//! An address has 10 hex digits, and the number takes the digits the base
//! address leaves: `ABAD1DEA` counts `ABAD1DEA01` to `ABAD1DEA99`, `ABAD1D`
//! counts `ABAD1D0001` to `ABAD1D9999`. The number is written in decimal
//! digits (`…09`, `…10`, never `…0A`), so it reads the same as in the name,
//! and names are zero padded to the same width (`CF-01`).
//!
//! Each new Crazyflie takes the lowest number from 1 whose name and address
//! are both free, so the number of a removed Crazyflie is used again, and a
//! name and an address handed out together have the same number.

use std::collections::HashSet;

use anyhow::{bail, Result};

use super::store::Swarm;
use crate::error::CliError;
use crate::utils::radio::RadioUri;

/// Hex digits in a radio address.
const ADDRESS_DIGITS: usize = 10;
/// How many digits names are padded to when no base address says.
const DEFAULT_WIDTH: usize = 2;
/// The address every Crazyflie comes with. It is never handed out.
const DEFAULT_ADDRESS: &str = "E7E7E7E7E7";
/// The base new Crazyflies are named with when the swarm has none.
pub const DEFAULT_BASE_NAME: &str = "CF-";

/// Check a base name, returned without the spaces around it.
pub fn check_base_name(base: &str) -> Result<String> {
    let base = base.trim();
    if base.contains("://") {
        bail!(CliError::InvalidValue(format!(
            "base name '{}' is a URI; give the URIs before --base-name, or write --base-name=<BASE>",
            base
        )));
    }
    if base.is_empty() {
        bail!(CliError::InvalidValue("a base name can't be empty".to_string()));
    }
    if base.contains(',') {
        bail!(CliError::InvalidValue(format!("base name '{}' contains a comma", base)));
    }
    Ok(base.to_string())
}

/// Check a base address: 1-9 hex digits, so at least one digit is left for
/// the number. Returned in upper case, without a `0x`.
pub fn check_base_address(base: &str) -> Result<String> {
    let trimmed = base.trim();
    if trimmed.contains("://") {
        bail!(CliError::InvalidValue(format!(
            "base address '{}' is a URI; give the URIs before --base-address, or write --base-address=<BASE>",
            trimmed
        )));
    }
    let digits = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")).unwrap_or(trimmed);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(CliError::InvalidValue(format!("base address '{}' must be hex digits, e.g. ABAD1DEA", base)));
    }
    if digits.len() >= ADDRESS_DIGITS {
        bail!(CliError::InvalidValue(format!(
            "base address '{}' leaves no digits for the number: an address has 10 hex digits, so a base of 8 \
             (ABAD1DEA) counts ABAD1DEA01 to ABAD1DEA99. Give an exact address with --set-address",
            base
        )));
    }
    Ok(digits.to_uppercase())
}

/// How many digits the number after a base address has.
pub fn width(base_address: &str) -> usize {
    ADDRESS_DIGITS - base_address.len()
}

/// How many digits names are padded to: as many as the number after the
/// base address in use, else after the swarm's base address, else 2.
pub fn name_width(base_address: Option<&str>, swarm: &Swarm) -> usize {
    base_address.or(swarm.base_address.as_deref()).map(width).unwrap_or(DEFAULT_WIDTH)
}

fn number(n: u64, width: usize) -> String {
    format!("{:0width$}", n, width = width)
}

/// The largest number with this many digits.
fn largest(width: usize) -> u64 {
    10u64.pow(width as u32) - 1
}

/// The addresses a base address hands out, e.g. `ABAD1DEA01 to ABAD1DEA99`.
pub fn address_range(base: &str) -> String {
    let width = width(base);
    format!("{}{} to {}{}", base, number(1, width), base, number(largest(width), width))
}

/// The first names a base name hands out, e.g. `CF-01, CF-02, ...`.
pub fn name_examples(base: &str, width: usize) -> String {
    format!("{}{}, {}{}, ...", base, number(1, width), base, number(2, width))
}

/// What a new Crazyflie gets from [`Numbering::next`].
#[derive(Debug, PartialEq, Eq)]
pub struct Numbered {
    /// Its name, when it is named from a base.
    pub name: Option<String>,
    /// Its address (10 hex digits), when it gets one from a base address.
    pub address: Option<String>,
}

/// Hands out the names and addresses of the Crazyflies added to a swarm.
pub struct Numbering {
    name_base: Option<String>,
    address_base: Option<String>,
    name_width: usize,
    /// The names in use, in lower case: names are unique ignoring case.
    names: HashSet<String>,
    /// The addresses in use on any channel, as 10 upper-case hex digits.
    addresses: HashSet<String>,
}

impl Numbering {
    /// Number new Crazyflies of `swarm`. Without a name base the names
    /// come from somewhere else (`--name`), without an address base the
    /// Crazyflies keep their addresses.
    pub fn new(swarm: &Swarm, name_base: Option<String>, address_base: Option<String>) -> Numbering {
        let name_width = name_width(address_base.as_deref(), swarm);
        Numbering {
            names: swarm.units.iter().map(|u| u.name.to_lowercase()).collect(),
            addresses: swarm
                .units
                .iter()
                .filter_map(|u| RadioUri::parse(&u.uri).ok().flatten())
                .map(|uri| uri.address)
                .collect(),
            name_base,
            address_base,
            name_width,
        }
    }

    /// The name and address of the next Crazyflie, from the lowest number
    /// for which both are free. They count as taken afterwards.
    pub fn next(&mut self) -> Result<Numbered> {
        if self.name_base.is_none() && self.address_base.is_none() {
            return Ok(Numbered { name: None, address: None });
        }
        // Names can grow past their padding (CF-100), addresses can't.
        let last = self.address_base.as_deref().map(|base| largest(width(base))).unwrap_or(u64::MAX);
        for n in 1..=last {
            let name = self.name_base.as_ref().map(|base| format!("{}{}", base, number(n, self.name_width)));
            let address = self.address_base.as_ref().map(|base| format!("{}{}", base, number(n, width(base))));
            if name.as_ref().is_some_and(|name| self.names.contains(&name.to_lowercase())) {
                continue;
            }
            if address.as_ref().is_some_and(|address| address == DEFAULT_ADDRESS || self.addresses.contains(address)) {
                continue;
            }
            if let Some(name) = &name {
                self.names.insert(name.to_lowercase());
            }
            if let Some(address) = &address {
                self.addresses.insert(address.clone());
            }
            return Ok(Numbered { name, address });
        }

        let base = self.address_base.as_deref().expect("only addresses run out");
        let taken = match &self.name_base {
            Some(_) => "or the name that goes with it, is taken",
            None => "is taken",
        };
        let mut message = format!("no free number for base address {}: every address from {}, {}", base, address_range(base), taken);
        if base.len() > 1 {
            message.push_str(&format!(
                "; a shorter base such as {} counts to {}",
                &base[..base.len() - 1],
                largest(width(base) + 1)
            ));
        }
        bail!(CliError::InvalidValue(message))
    }

    /// Count a name given with `--name` as taken.
    pub fn take_name(&mut self, name: &str) {
        self.names.insert(name.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::swarm::store::Unit;

    fn swarm(units: &[(&str, &str)]) -> Swarm {
        let mut swarm = Swarm::new("test".to_string(), None);
        for (uri, name) in units {
            swarm.units.push(Unit {
                uri: uri.to_string(),
                name: name.to_string(),
                description: None,
                extra: Default::default(),
            });
        }
        swarm
    }

    fn numbering(swarm: &Swarm, name_base: Option<&str>, address_base: Option<&str>) -> Numbering {
        Numbering::new(swarm, name_base.map(String::from), address_base.map(String::from))
    }

    fn next(numbering: &mut Numbering) -> (Option<String>, Option<String>) {
        let numbered = numbering.next().unwrap();
        (numbered.name, numbered.address)
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn the_base_address_leaves_the_rest_of_the_digits_for_the_number() {
        assert_eq!(width("ABAD1DEA"), 2);
        assert_eq!(width("ABAD1D"), 4);
        assert_eq!(address_range("ABAD1DEA"), "ABAD1DEA01 to ABAD1DEA99");
        assert_eq!(address_range("ABAD1DE"), "ABAD1DE001 to ABAD1DE999");
    }

    #[test]
    fn checks_base_addresses() {
        assert_eq!(check_base_address("abad1dea").unwrap(), "ABAD1DEA");
        assert_eq!(check_base_address("0xABAD1DEA").unwrap(), "ABAD1DEA");
        assert_eq!(check_base_address("E").unwrap(), "E");
        for bad in ["", "ABAD1DEAG", "ABAD1DEA01", "radio:///80/2M/E7E7E7E701"] {
            assert!(check_base_address(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn checks_base_names() {
        assert_eq!(check_base_name(" CF- ").unwrap(), "CF-");
        for bad in ["", "  ", "CF,", "radio:///80/2M/E7E7E7E701"] {
            assert!(check_base_name(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn numbers_look_decimal_in_the_address_and_the_name() {
        let swarm = swarm(&[]);
        let mut numbering = numbering(&swarm, Some("CF-"), Some("ABAD1DEA"));
        for _ in 1..10 {
            numbering.next().unwrap();
        }
        assert_eq!(next(&mut numbering), (some("CF-10"), some("ABAD1DEA10")));
    }

    #[test]
    fn fills_the_gap_a_removed_crazyflie_left() {
        let swarm = swarm(&[("radio:///90/2M/ABAD1DEA01", "CF-01"), ("radio:///90/2M/ABAD1DEA03", "CF-03")]);
        let mut numbering = numbering(&swarm, Some("CF-"), Some("ABAD1DEA"));
        assert_eq!(next(&mut numbering), (some("CF-02"), some("ABAD1DEA02")));
        assert_eq!(next(&mut numbering), (some("CF-04"), some("ABAD1DEA04")));
    }

    #[test]
    fn a_name_and_an_address_together_take_a_number_where_both_are_free() {
        // CF-01 has an old address, Rig has ABAD1DEA02.
        let swarm = swarm(&[("radio:///80/2M/E7E7E7E701", "CF-01"), ("radio:///90/2M/ABAD1DEA02", "Rig")]);
        let mut numbering = numbering(&swarm, Some("CF-"), Some("ABAD1DEA"));
        assert_eq!(next(&mut numbering), (some("CF-03"), some("ABAD1DEA03")));
    }

    #[test]
    fn without_a_base_address_the_name_numbers_on_its_own() {
        let swarm = swarm(&[("radio:///80/2M/E7E7E7E701", "CF-01")]);
        let mut numbering = numbering(&swarm, Some("cf-"), None);
        // Names are unique ignoring case.
        assert_eq!(next(&mut numbering), (some("cf-02"), None));
    }

    #[test]
    fn with_a_given_name_only_the_address_counts() {
        let swarm = swarm(&[("radio:///80/2M/E7E7E7E701", "CF-01")]);
        let mut numbering = numbering(&swarm, None, Some("ABAD1DEA"));
        assert_eq!(next(&mut numbering), (None, some("ABAD1DEA01")));
    }

    #[test]
    fn addresses_count_on_every_channel() {
        let swarm = swarm(&[("radio:///80/2M/ABAD1DEA01", "A"), ("radio:///76/2M/abad1dea02", "B")]);
        let mut numbering = numbering(&swarm, None, Some("ABAD1DEA"));
        assert_eq!(next(&mut numbering), (None, some("ABAD1DEA03")));
    }

    #[test]
    fn names_are_padded_as_wide_as_the_number_in_the_address() {
        let mut stored = swarm(&[]);
        assert_eq!(next(&mut numbering(&stored, Some("CF-"), Some("ABAD1D"))), (some("CF-0001"), some("ABAD1D0001")));
        // The swarm's base address sets the width when none is used.
        stored.base_address = some("ABAD1DE");
        assert_eq!(next(&mut numbering(&stored, Some("CF-"), None)), (some("CF-001"), None));
        assert_eq!(next(&mut numbering(&swarm(&[]), Some("CF-"), None)), (some("CF-01"), None));
    }

    #[test]
    fn names_grow_past_their_padding() {
        let units: Vec<(String, String)> =
            (1..=99).map(|n| (format!("radio:///80/2M/E7E7E7E7{:02}", n), format!("CF-{:02}", n))).collect();
        let refs: Vec<(&str, &str)> = units.iter().map(|(u, n)| (u.as_str(), n.as_str())).collect();
        let swarm = swarm(&refs);
        assert_eq!(next(&mut numbering(&swarm, Some("CF-"), None)), (some("CF-100"), None));
    }

    #[test]
    fn a_full_range_is_an_error_naming_a_shorter_base() {
        let mut numbering = numbering(&swarm(&[]), None, Some("ABAD1DEAB"));
        for _ in 1..=9 {
            numbering.next().unwrap();
        }
        let error = numbering.next().unwrap_err().to_string();
        assert!(error.contains("ABAD1DEAB1 to ABAD1DEAB9"), "{}", error);
        assert!(error.contains("ABAD1DEA counts to 99"), "{}", error);
    }

    #[test]
    fn never_hands_out_the_default_address() {
        let mut numbering = numbering(&swarm(&[]), None, Some("E7E7E7E7E"));
        // 1-9 without 7 leaves 8 addresses.
        let handed_out: Vec<String> = (1..=8).map(|_| numbering.next().unwrap().address.unwrap()).collect();
        assert!(handed_out.contains(&"E7E7E7E7E8".to_string()));
        assert!(!handed_out.contains(&"E7E7E7E7E7".to_string()));
        assert!(numbering.next().is_err());
    }

    #[test]
    fn given_names_count_as_taken() {
        let mut numbering = numbering(&swarm(&[]), Some("CF-"), None);
        numbering.take_name("CF-01");
        assert_eq!(next(&mut numbering), (some("CF-02"), None));
    }
}
