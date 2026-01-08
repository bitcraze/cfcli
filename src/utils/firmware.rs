use std::collections::HashMap;
use serde::{Deserialize, Serialize, Deserializer};

fn string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct StringOrVec;

    impl<'de> Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileInfo {
  platform: String,
  #[serde(deserialize_with = "string_or_vec")]
  target: Vec<String>,
  #[serde(rename = "type")]
  file_type: String,
  release: String,
  repository: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  requires: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  provides: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
  version: u32,
  subversion: u32,
  fw_platform: String,
  release: String,
  files: HashMap<String, FileInfo>,
}

#[derive(Clone)]
pub struct Firmware {
    pub data: Vec<u8>,
    pub file_name: String,
    pub target: String,
    pub version: String,
    pub file_type: String,
}

impl std::fmt::Debug for Firmware {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Firmware")
      .field("file_name", &self.file_name)
      .field("target", &self.target)
      .field("version", &self.version)
      .field("file_type", &self.file_type)
      .field("data_size", &self.data.len())
      .finish()
  }
}

struct FirmwareArchive {
    manifest: Manifest,
    binaries: HashMap<String, Firmware>,
}

impl FirmwareArchive {

    fn manifest_from_bytes(data: &[u8]) -> Result<Manifest, Box<dyn std::error::Error>> {
        let manifest: Manifest = serde_json::from_slice(data)?;
        Ok(manifest)
    }

    pub fn from_zip(data: Vec<u8>) -> Result<Self, Box<dyn std::error::Error>> {
        // Extract binaries from zip_data
        let cursor = std::io::Cursor::new(data);
        let mut archive = zip::ZipArchive::new(cursor)?;

        let mut extracted_bins = HashMap::new();

        let mut manifest_file = archive.by_name("manifest.json")?;
        let mut manifest_data = Vec::new();
        std::io::Read::read_to_end(&mut manifest_file, &mut manifest_data)?;
        drop(manifest_file);

        let manifest = Self::manifest_from_bytes(&manifest_data)?;

        if manifest.version != 2 || manifest.subversion != 1 {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unsupported manifest version: {}.{}", manifest.version, manifest.subversion),
            )));
        }

        for i in 0..archive.len() {
          let mut file = archive.by_index(i)?;
          if file.is_file() && file.name() != "manifest.json" {
            let mut buffer = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut buffer)?;
            
            let file_info = manifest.files.get(file.name()).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("File {} not found in manifest", file.name()),
                )
            })?;

            for target in &file_info.target {
              let target_and_type = format!("{}-{}", target, file_info.file_type);

              extracted_bins.insert(target_and_type, Firmware {
                  data: buffer.clone(),
                  file_name: file.name().to_string(),
                  target: target.to_string(),
                  version: file_info.release.to_string(),
                  file_type: file_info.file_type.to_string(),
              });
            }
          }
        }

        Ok(FirmwareArchive {
            manifest: manifest,
            binaries: extracted_bins,
        })
    }
}

async fn download_release_zip(release: &String, platform_release: &String) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Placeholder for downloading and extracting firmware binaries from a release zip

    let release_name = format!("firmware-{}-{}.zip", platform_release, release);
    println!("Downloading release: {}", release_name);

    let octocrab = octocrab::Octocrab::builder().build()?;
    let release_obj = octocrab
      .repos("bitcraze", "crazyflie-release")
      .releases()
      .get_by_tag(&release)
      .await?;

    // Find the zip asset
    let asset = release_obj
      .assets
      .iter()
      .find(|a| a.name == release_name)
      .ok_or_else(|| {
        std::io::Error::new(
          std::io::ErrorKind::NotFound,
          format!("Asset {} not found in release", release_name),
        )
      })?;

    // Download the asset
    let response = reqwest::get(asset.browser_download_url.clone()).await?;
    let zip_data = response.bytes().await?.to_vec();

    Ok(zip_data)
}

pub struct FirmwareUpgrade {
    bins: HashMap<String, Firmware>,
}

impl FirmwareUpgrade {

    fn convert_platform_to_release_name(platform: &String) -> Result<String, Box<dyn std::error::Error>> {
        match platform.as_str() {
            "Crazyflie Bolt 1.1" => Ok("bolt".to_string()),
            "Crazyflie 2.1" => Ok("cf2".to_string()),
            "Crazyflie 2.1 Brushless" => Ok("cf21bl".to_string()),
            "Flapper (Bolt 1.1)" => Ok("flapper".to_string()),
            "Roadrunner 1.0" => Ok("tag".to_string()),
            _ => Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Unknown platform: {}", platform),
            ))),
        }
    }

    pub fn get_target_and_types(&self) -> Vec<String> {
        self.bins.keys().cloned().collect()
    }

    // pub fn get_firmware_for_target(&self, target: &String) -> Option<&Firmware> {
    //     self.bins.get(target)
    // }

    pub fn get_firmware_for_bootloader(&self) -> Vec<Firmware> {
        self.bins
            .values()
            .filter(|fw| fw.target == "stm32" || fw.target == "nrf51")
            .cloned()
            .collect()
    }

    pub fn get_firmware_for_decks(&self) -> Vec<Firmware> {
        self.bins
            .values()
            .filter(|fw| fw.target != "stm32" && fw.target != "nrf51")
            .cloned()
            .collect()
    }

    pub fn filter_targets(&mut self, selected: &Vec<String>) {
        self.bins.retain(|key, _| selected.contains(key));
    }

    pub async fn new(platform: &String, release: &Option<String>, zip: &Option<String>, bin: &Option<HashMap<String, String>>) -> Result<Self, Box<dyn std::error::Error>> {

        let platform_release = Self::convert_platform_to_release_name(platform)?;

        let mut bins = match (release, zip) {
          (Some(release), None) => {
              // Download release zip and extract binaries
              let zip_data = download_release_zip(&release, &platform_release).await?;
              let archive = FirmwareArchive::from_zip(zip_data)?;

              if archive.manifest.fw_platform != *platform_release {
                  return Err(Box::new(std::io::Error::new(
                      std::io::ErrorKind::InvalidData,
                      format!("Release platform {} does not match connected platform {}", archive.manifest.fw_platform, platform_release),
                  )));
              }

              archive.binaries
          },
          (None, Some(zip_path)) => {
              // Read zip file from path and extract binaries
              let zip_data = std::fs::read(zip_path)?;
              let archive = FirmwareArchive::from_zip(zip_data)?;

              if archive.manifest.fw_platform != *platform_release {
                  return Err(Box::new(std::io::Error::new(
                      std::io::ErrorKind::InvalidData,
                      format!("Release platform {} does not match connected platform {}", archive.manifest.fw_platform, platform_release),
                  )));
              }
              archive.binaries
          },
          _ => {
              HashMap::new()
          }
        };

        // Overload with the supplied binaries if any
        // dbg!(&bin);
        if let Some(bin_map) = bin {
            for (key, path) in bin_map {
                let data = std::fs::read(path)?;
                let parts: Vec<&str> = key.split('-').collect();
                let target = parts.get(0).unwrap().to_string();
                let file_type = parts.get(1).unwrap_or(&"fw").to_string();
                bins.insert(key.clone(), Firmware {
                    data,
                    file_name: path.clone(),
                    target,
                    version: "custom".to_string(),
                    file_type,
                });
            }
        }

        // dbg!(&bins);

        Ok(FirmwareUpgrade {
            bins
        })
    }
}

async fn get_releases() -> Result<Vec<octocrab::models::repos::Release>, Box<dyn std::error::Error>> {
    let octocrab = octocrab::Octocrab::builder().build()?;
    let releases = octocrab
        .repos("bitcraze", "crazyflie-release")
        .releases()
        .list()
        .per_page(10)
        .send()
        .await?;

    Ok(releases.items)
}

pub async fn get_release_labels() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let releases = get_releases().await?;
    let labels = releases
        .iter()
        .filter_map(|r| r.name.clone())
        .collect::<Vec<String>>();

    Ok(labels)
}

pub async fn print_releases() -> Result<(), Box<dyn std::error::Error>> {
    let releases = get_releases().await?;

    println!("Latest Crazyflie firmware releases:");
    for release in releases {
        println!(
            "- {}\t{}",
            release.name.unwrap_or_else(|| "Unnamed release".to_string()),
            release.published_at.map(|dt| dt.to_string()).unwrap_or_else(|| "Unknown date".to_string())
        );
    }

    Ok(())
}
