//! `udp://<host>:<port>` URIs reach the Crazyflie firmware simulator
//! (Simmyflie). It runs the STM32 firmware only: there is no nRF51 to take
//! power commands and no bootloader to flash through.

use anyhow::{bail, Result};

/// Fail for a `udp://` URI: `what` needs the nRF51 or the bootloader.
pub fn check_not_udp(uri: &str, what: &str) -> Result<()> {
    if uri.starts_with("udp://") {
        bail!("{} is not supported over udp://, the firmware simulator has no nRF51 or bootloader", what);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_not_udp_rejects_udp_only() {
        assert!(check_not_udp("udp://localhost:50000", "Rebooting").is_err());
        assert!(check_not_udp("radio://0/80/2M/E7E7E7E7E7", "Rebooting").is_ok());
        assert!(check_not_udp("usb://0", "Rebooting").is_ok());
    }
}
