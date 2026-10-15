//! What `bootload flash` and `swarm bootload flash` flash, from their
//! arguments: a release, a zip, binary files and the targets.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use inquire::{Confirm, MultiSelect, Select};

use crate::error::CliError;
use crate::modules::bootloader;
use crate::utils::firmware::{self, FirmwareUpgrade};

/// The platforms `--platform` takes: the short name, and the name the
/// Crazyflie reports.
const PLATFORMS: [(&str, &str); 5] = [
    ("cf21", "Crazyflie 2.1"),
    ("cf21bl", "Crazyflie 2.1 Brushless"),
    ("bolt11", "Crazyflie Bolt 1.1"),
    ("flapper", "Flapper (Bolt 1.1)"),
    ("tag", "Roadrunner 1.0"),
];

/// The name a Crazyflie reports for a `--platform` short name.
pub fn platform_name(short: &str) -> Result<&'static str> {
    PLATFORMS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(short))
        .map(|(_, platform)| *platform)
        .ok_or_else(|| {
            let valid: Vec<&str> = PLATFORMS.iter().map(|(name, _)| *name).collect();
            anyhow!(CliError::InvalidValue(format!(
                "unknown platform '{}'. Valid options: {}",
                short,
                valid.join(", ")
            )))
        })
}

/// The names Crazyflies report, to pick from.
pub fn platform_names() -> Vec<&'static str> {
    PLATFORMS.iter().map(|(_, platform)| *platform).collect()
}

/// The `--bin` keys for the STM32 or the nRF51, whose images are built for
/// one platform. Deck firmware works whatever the Crazyflie.
pub fn platform_bound(bins: &Option<HashMap<String, String>>) -> Vec<String> {
    let mut keys: Vec<String> = bins
        .iter()
        .flatten()
        .map(|(key, _)| key)
        .filter(|key| matches!(key.split(['-', '@']).next(), Some("stm32") | Some("nrf51")))
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The release to flash: the one named (checked to exist), one picked from
/// the list, or none.
pub async fn release(release: &Option<Option<String>>, non_interactive: bool) -> Result<Option<String>> {
    Ok(match release {
        Some(Some(r)) => {
            let labels = firmware::get_release_labels().await?;
            if !labels.contains(r) {
                bail!(CliError::NotFound(format!("release '{}'", r)));
            }
            Some(r.clone())
        }
        Some(None) => {
            crate::require_arg(non_interactive, "--release <NAME>")?;
            let labels = firmware::get_release_labels().await?;
            let selected_release = Select::new("Select a firmware release to flash:", labels)
                .prompt()
                .map_err(|_| anyhow!("No release selected"))?;
            Some(selected_release)
        }
        None => None,
    })
}

/// The binary files to flash, by target. A `--bin` given without a target
/// gets the single `--targets` one, or one picked from the list.
pub fn bins(
    bin: &Option<HashMap<String, Option<String>>>,
    targets: &Option<Option<String>>,
    non_interactive: bool,
) -> Result<Option<HashMap<String, String>>> {
    // This case is special since we're not setting the key on the command-line,
    // we're actually setting the value and then we'll select they key here
    // Note that the list of tarets is hardcoded, this is because we cannot
    // query the Crazyflie for it, flashing new firmware might change this
    // until we reach the deck flashing stage.
    let mut result = HashMap::new();
    let target_from_single_bare_bin = single_explicit_target_for_bare_bin(bin, targets);
    if let Some(bin_map) = bin {
        for (key, value_opt) in bin_map.iter() {
            let (k, v) = match (key, value_opt) {
                (k, Some(v)) => (k.clone(), v.clone()),
                (k, None) => {
                    if let Some(selected_target) = &target_from_single_bare_bin {
                        (selected_target.clone(), k.to_string())
                    } else {
                        crate::require_arg(
                            non_interactive,
                            "--bin target=file (or: --targets <TARGET> for a single bare --bin)",
                        )?;
                        let selected_target = Select::new(
                            &format!("Select target for [{}]:", k),
                            bootloader::get_hardcoded_list_of_targets(),
                        )
                        .prompt()
                        .map_err(|_| anyhow!("No binary selected"))?;
                        (selected_target.to_string(), k.to_string())
                    }
                }
            };
            result.insert(k, v);
        }
    }
    Ok(Some(result))
}

/// The targets to flash: the ones given (checked to be known), ones picked
/// from those in `upgrade`, or all of them.
pub fn targets(
    upgrade: &FirmwareUpgrade,
    targets: &Option<Option<String>>,
    non_interactive: bool,
) -> Result<Vec<String>> {
    let selected = match targets {
        Some(Some(t)) => t.split(',').map(|s| s.trim().to_string()).collect(),
        Some(None) => {
            crate::require_arg(non_interactive, "--targets <list>")?;
            MultiSelect::new("Select targets to flash:", upgrade.get_target_and_types())
                .prompt()
                .map_err(|_| anyhow!("No targets selected"))?
        }
        None => upgrade.get_target_and_types(),
    };

    if matches!(targets, Some(Some(_))) {
        let unsupported_targets = unsupported_flash_targets(&selected);
        if !unsupported_targets.is_empty() {
            bail!(CliError::InvalidValue(format!(
                "unknown flash target(s): {}. Valid targets: {}",
                unsupported_targets.join(", "),
                bootloader::get_hardcoded_list_of_targets().join(", ")
            )));
        }
    }
    Ok(selected)
}

/// Asks before an nRF51 bootloader+softdevice given with --bin is flashed.
/// The one in a release or zip is a released bootloader, but a file can be
/// anything, and a Crazyflie whose nRF51 bootloader does not start can only
/// be recovered with an SWD debug probe. `crazyflies` is how many would get
/// it. Returns whether to go on.
pub fn confirm_nrf51_bootloader(
    upgrade: &FirmwareUpgrade,
    crazyflies: usize,
    accepted: bool,
    non_interactive: bool,
) -> Result<bool> {
    let Some(bundle) = upgrade.nrf51_softdevice_from_bin() else {
        return Ok(true);
    };
    if accepted {
        return Ok(true);
    }

    eprintln!(
        "Warning: this replaces the nRF51 bootloader with {}. If that bootloader does not start, \
         the Crazyflie can only be recovered with an SWD debug probe.",
        bundle.file_name
    );
    crate::require_arg(non_interactive, "--accept-bootloader-risk")?;
    let question = if crazyflies == 1 {
        "Flash the nRF51 bootloader?".to_string()
    } else {
        format!("Flash the nRF51 bootloader to {} Crazyflies?", crazyflies)
    };
    Ok(Confirm::new(&question).with_default(false).prompt().unwrap_or(false))
}

fn single_explicit_target_for_bare_bin(
    bin: &Option<HashMap<String, Option<String>>>,
    targets: &Option<Option<String>>,
) -> Option<String> {
    let bin_map = bin.as_ref()?;
    if bin_map.len() != 1 {
        return None;
    }

    let (_bin_path, selected_target) = bin_map.iter().next()?;
    if selected_target.is_some() {
        return None;
    }

    let target_arg = match targets {
        Some(Some(target_arg)) => target_arg,
        _ => return None,
    };

    let mut target_names = target_arg
        .split(',')
        .map(str::trim)
        .filter(|target| !target.is_empty());
    let target = target_names.next()?;
    if target_names.next().is_some() {
        return None;
    }

    Some(target.to_string())
}

fn unsupported_flash_targets(selected: &[String]) -> Vec<String> {
    let supported = bootloader::get_hardcoded_list_of_targets();

    selected
        .iter()
        .filter(|target| !supported.contains(&target.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::firmware::tests::binary;

    #[test]
    fn single_bare_bin_uses_single_explicit_target() {
        let mut bin = HashMap::new();
        bin.insert("lighthouse.bin".to_string(), None);
        let targets = Some(Some("bcLighthouse4-fw".to_string()));

        assert_eq!(
            single_explicit_target_for_bare_bin(&Some(bin), &targets),
            Some("bcLighthouse4-fw".to_string())
        );
    }

    #[test]
    fn single_bare_bin_does_not_use_multiple_explicit_targets() {
        let mut bin = HashMap::new();
        bin.insert("lighthouse.bin".to_string(), None);
        let targets = Some(Some("bcLighthouse4-fw,stm32-fw".to_string()));

        assert_eq!(single_explicit_target_for_bare_bin(&Some(bin), &targets), None);
    }

    #[test]
    fn keyed_bin_does_not_get_rewritten_from_targets() {
        let mut bin = HashMap::new();
        bin.insert("bcLighthouse4-fw".to_string(), Some("lighthouse.bin".to_string()));
        let targets = Some(Some("bcLighthouse4-fw".to_string()));

        assert_eq!(single_explicit_target_for_bare_bin(&Some(bin), &targets), None);
    }

    #[test]
    fn platform_short_names() {
        assert_eq!(platform_name("cf21bl").unwrap(), "Crazyflie 2.1 Brushless");
        assert_eq!(platform_name("CF21").unwrap(), "Crazyflie 2.1");
        assert!(platform_name("cf3").is_err());
    }

    #[test]
    fn stm32_and_nrf51_images_are_platform_bound() {
        let bins: HashMap<String, String> = [
            ("stm32-fw", "a.bin"),
            ("nrf51-fw", "b.bin"),
            ("stm32-fw@0x08004000", "c.bin"),
            ("bcLighthouse4-fw", "d.bin"),
            ("deckctrl-cfg", "e.bin"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(platform_bound(&Some(bins)), vec!["nrf51-fw", "stm32-fw", "stm32-fw@0x08004000"]);
        assert!(platform_bound(&None).is_empty());
    }

    #[test]
    fn unsupported_flash_targets_reports_unknown_targets() {
        let selected = vec!["stm32ohnooo-fw".to_string(), "stm32-fw".to_string()];

        assert_eq!(unsupported_flash_targets(&selected), vec!["stm32ohnooo-fw".to_string()]);
    }

    #[test]
    fn released_nrf51_bootloader_is_flashed_without_asking() {
        let upgrade = FirmwareUpgrade::from_binaries(vec![binary(
            "nrf51",
            firmware::NRF51_SOFTDEVICE_TYPE,
            Some("2026.08"),
        )]);
        assert!(confirm_nrf51_bootloader(&upgrade, 1, false, true).unwrap());
    }

    #[test]
    fn nrf51_bootloader_from_bin_needs_accepting_non_interactively() {
        let upgrade = FirmwareUpgrade::from_binaries(vec![binary("nrf51", firmware::NRF51_SOFTDEVICE_TYPE, None)]);
        let refused = confirm_nrf51_bootloader(&upgrade, 1, false, true).unwrap_err();
        assert!(matches!(refused.downcast_ref::<CliError>(), Some(CliError::MissingArg(_))));
        assert!(confirm_nrf51_bootloader(&upgrade, 1, true, true).unwrap());
    }
}
