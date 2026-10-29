//! Platform information for cfcli.

use anyhow::Result;
use crazyflie_connection_server::Crazyflie;
use tabled::Tabled;

/// What `platform info` shows.
#[derive(Tabled)]
pub struct PlatformInfo {
    #[tabled(rename = "Platform")]
    pub platform: String,
    #[tabled(rename = "Firmware")]
    pub firmware: String,
    #[tabled(rename = "CRTP protocol")]
    pub protocol: String,
}

impl PlatformInfo {
    /// The CSV columns, in the order of [`PlatformInfo::csv_fields`].
    pub const CSV_HEADER: [&'static str; 3] = ["platform", "firmware", "crtp_protocol"];

    pub fn csv_fields(&self) -> [&str; 3] {
        [&self.platform, &self.firmware, &self.protocol]
    }
}

/// Read when the Crazyflie was connected, so this needs no radio traffic.
pub async fn info(cf: &Crazyflie) -> Result<PlatformInfo> {
    let unknown = || "?".to_string();
    Ok(PlatformInfo {
        platform: cf.platform.device_type_name().map_or_else(unknown, str::to_string),
        firmware: cf.platform.firmware_version().map_or_else(unknown, str::to_string),
        protocol: cf.platform.protocol_version().map_or_else(unknown, |version| version.to_string()),
    })
}
