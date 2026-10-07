use std::fmt;

/// Classified errors that map to non-zero exit codes the CLI promises to its
/// callers (humans and scripts/agents). The `exit_code` is the contract; the
/// human-readable message is best-effort and can change.
#[derive(Debug)]
pub enum CliError {
    Connection(String),
    NotFound(String),
    /// A required argument wasn't supplied and the CLI is non-interactive
    /// (or `--non-interactive` was passed). Shares exit code 30 with
    /// `InvalidValue` — both are the caller's responsibility to fix.
    MissingArg(String),
    InvalidValue(String),
    Timeout(String),
    /// A swarm command failed on some of its Crazyflies but not on all of
    /// them (or on all of them for different reasons).
    SomeFailed(String),
    /// A check found that the Crazyflie differs from what it was compared
    /// with (`lh config check`).
    Differs(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Connection(_) => 10,
            CliError::NotFound(_) => 20,
            CliError::MissingArg(_) | CliError::InvalidValue(_) => 30,
            CliError::Timeout(_) => 40,
            CliError::SomeFailed(_) => 50,
            CliError::Differs(_) => 60,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Connection(s) => write!(f, "connection error: {}", s),
            CliError::NotFound(s) => write!(f, "not found: {}", s),
            CliError::MissingArg(s) => write!(f, "missing argument: {}", s),
            CliError::InvalidValue(s) => write!(f, "invalid value: {}", s),
            CliError::Timeout(s) => write!(f, "timeout: {}", s),
            CliError::SomeFailed(s) => write!(f, "some Crazyflies failed: {}", s),
            CliError::Differs(s) => write!(f, "differs: {}", s),
        }
    }
}

impl std::error::Error for CliError {}

/// Walk the anyhow error chain and return the most specific exit code we can
/// derive. Explicit `CliError` annotations win; otherwise we map known
/// `crazyflie_lib::Error` variants to the appropriate bucket. Unclassified
/// failures return `1`.
pub fn classify_exit_code(err: &anyhow::Error) -> i32 {
    // Downcasting the anyhow error itself, rather than the `chain()` items, is
    // what finds a `CliError` attached with `.context()`: the chain only
    // exposes anyhow's own wrapper type there, while this traverses contexts.
    if let Some(cli) = err.downcast_ref::<CliError>() {
        return cli.exit_code();
    }
    for cause in err.chain() {
        if let Some(cli) = cause.downcast_ref::<CliError>() {
            return cli.exit_code();
        }
        if let Some(cf_err) = cause.downcast_ref::<crazyflie_lib::Error>() {
            // We rely on typed variants only. `ParamError`/`LogError` carry
            // free-form strings (including missing-name errors) — the modules
            // pre-check known names and raise `CliError::NotFound` themselves
            // before these strings can bubble up here.
            match cf_err {
                crazyflie_lib::Error::VariableNotFound => return 20,
                crazyflie_lib::Error::LinkError(_)
                | crazyflie_lib::Error::Disconnected => return 10,
                crazyflie_lib::Error::InvalidArgument(_)
                | crazyflie_lib::Error::InvalidParameter(_)
                | crazyflie_lib::Error::ConversionError(_) => return 30,
                _ => {}
            }
        }
    }
    1
}

/// Extra guidance printed after the error message when the failure has a
/// known way out. Walks the anyhow chain looking for typed errors we have
/// concrete advice for; returns `None` when we have nothing useful to add.
pub fn hint(err: &anyhow::Error) -> Option<String> {
    for cause in err.chain() {
        if let Some(crazyflie_lib::Error::ProtocolVersionNotSupported {
            min_supported,
            max_supported,
            found,
        }) = cause.downcast_ref::<crazyflie_lib::Error>()
        {
            // Firmware newer than this cfcli: updating the firmware would only
            // make the gap worse, the CLI is the side that has to move.
            if found > max_supported {
                return Some(format!(
                    "the firmware is newer than this cfcli (CRTP {} against a supported {}-{}).\n      \
                     Update cfcli rather than the Crazyflie.",
                    found, min_supported, max_supported
                ));
            }

            // Firmware too old. We cannot ask it to reboot into its bootloader
            // over CRTP, since that is exactly the link that just failed, so
            // the bootloader has to be entered by hand and flashed cold.
            return Some(
                "the firmware is too old for this cfcli. It cannot be asked to reboot into\n      \
                 its bootloader over a link that will not come up, so flash it cold:\n\n        \
                 cfcli bootload flash --release --cold\n\n      \
                 Power the Crazyflie off, then hold the power button for a few seconds\n      \
                 until the blue LEDs blink to enter the bootloader before flashing."
                    .to_string(),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protocol_error(found: u8) -> anyhow::Error {
        // Built the same way `connect_cf` builds it, so the test also covers
        // that the typed error survives the `CliError::Connection` context.
        anyhow::Error::new(crazyflie_lib::Error::ProtocolVersionNotSupported {
            min_supported: 12,
            max_supported: 13,
            found,
        })
        .context(CliError::Connection(
            "connecting to usb://4A002B0007504B5957333720".to_string(),
        ))
    }

    #[test]
    fn old_firmware_is_pointed_at_a_cold_boot_flash() {
        let hint = hint(&protocol_error(7)).expect("old firmware should get a hint");
        assert!(hint.contains("cfcli bootload flash --release --cold"), "{}", hint);
        assert!(hint.contains("too old for this cfcli"), "{}", hint);
    }

    #[test]
    fn new_firmware_is_pointed_at_a_cfcli_update() {
        let hint = hint(&protocol_error(20)).expect("new firmware should get a hint");
        assert!(hint.contains("Update cfcli"), "{}", hint);
        assert!(hint.contains("CRTP 20 against a supported 12-13"), "{}", hint);
        assert!(!hint.contains("--cold"), "{}", hint);
    }

    #[test]
    fn unrelated_errors_get_no_hint() {
        let err = anyhow::Error::new(CliError::Connection("no USB Crazyflies found".to_string()));
        assert!(hint(&err).is_none());
    }

    #[test]
    fn connection_context_still_renders_the_underlying_error() {
        assert_eq!(
            format!("{:#}", protocol_error(7)),
            "connection error: connecting to usb://4A002B0007504B5957333720: \
             Protocol version not supported: supported range is 12-13, found 7"
        );
    }

    #[test]
    fn protocol_mismatch_still_exits_as_a_connection_error() {
        assert_eq!(classify_exit_code(&protocol_error(7)), 10);
    }
}
