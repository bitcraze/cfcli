//! Platform information for cfcli.

use anyhow::Result;
use crazyflie_lib::Crazyflie;
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

pub async fn info(cf: &Crazyflie) -> Result<PlatformInfo> {
    Ok(PlatformInfo {
        platform: cf.platform.device_type_name().await?,
        firmware: cf.platform.firmware_version().await?,
        protocol: cf.platform.protocol_version().await?.to_string(),
    })
}
