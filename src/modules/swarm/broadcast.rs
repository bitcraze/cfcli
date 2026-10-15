//! Broadcast flashing for `cfcli swarm bootload flash`.
//!
//! Every Crazyflie with STM32 or nRF51 firmware to flash is restarted into its
//! bootloader first, which is also where its bootloader versions are read.
//! The Crazyflies of one platform whose nRF51 and STM32 bootloaders both take
//! broadcast (protocol 0x11) are armed for it and receive each image once, all
//! at the same time. Each one is then checked by CRC, and the pages that
//! didn't make it are flashed again over unicast. The other Crazyflies are
//! flashed one at a time from their bootloader. Deck firmware is flashed last,
//! the same way `cfcli bootload flash` does it, one Crazyflie per Crazyradio
//! at a time, all Crazyradios at once.
//!
//! A broadcast waits for the slowest possible flash erase after every chunk,
//! so for a single Crazyflie it takes longer than unicast. A platform with
//! only one Crazyflie that could take a broadcast is flashed over unicast.
//!
//! In the bootloader every Crazyflie listens on channel 0, so all of this
//! runs on one Crazyradio; only the restarts into the bootloader are spread
//! over the Crazyradios. At the end every Crazyflie is checked to have left
//! its bootloader: one that stays there only answers at its bootloader
//! address, where the swarm commands can't reach it.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use cfloader::bootloader::{TARGET_NRF51, TARGET_STM32};
use cfloader::packets::InfoPacket;
use cfloader::{Bllink, CFLoader, BROADCAST_ADDRESS};
use crazyflie_lib::crazyflie_link::SharedCrazyradio;
use tokio::time::sleep;

use super::bootload::{BOOTLOADER_PER_RADIO, BROADCAST_PROTOCOL};
use super::runner::{progress, Runner};
use crate::modules::bootloader::{self, DeckOutput};
use crate::utils::display::{finish_progress, get_progressbar};
use crate::utils::firmware::{Firmware, FirmwareUpgrade};
use crate::ConfigTocCache;

/// Fewest Crazyflies a broadcast is used for.
const MIN_BROADCAST: usize = 2;

/// Rounds of CRC check and re-flash of the pages that differ.
const FIX_ATTEMPTS: usize = 3;

/// Crazyflies whose decks are flashed at the same time on each Crazyradio.
/// Writing deck firmware keeps a Crazyradio busy, like unicast flashing.
const DECKS_PER_RADIO: usize = 1;

/// How long the Crazyflies get to start their firmware before their decks
/// are flashed. The nRF51 delays the start by 5 s when an AI-deck is attached.
const RESTART_DELAY: Duration = Duration::from_secs(7);

/// Attempts at getting the Crazyradio for the bootloaders. The links that
/// restarted the Crazyflies only just let go of it, and it is closed on a
/// background thread.
const RADIO_ATTEMPTS: usize = 10;
const RADIO_RETRY: Duration = Duration::from_millis(300);

/// One Crazyflie to flash.
pub struct Job<'u> {
    /// Its index in the runner's targets.
    pub index: usize,
    pub platform: String,
    pub upgrade: &'u FirmwareUpgrade,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chip {
    Stm32,
    Nrf51,
}

impl Chip {
    fn target(self) -> u8 {
        match self {
            Chip::Stm32 => TARGET_STM32,
            Chip::Nrf51 => TARGET_NRF51,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Chip::Stm32 => "STM32",
            Chip::Nrf51 => "nRF51",
        }
    }

    /// The label of its progress bars, the same as `cfcli bootload flash`.
    fn label(self) -> &'static str {
        match self {
            Chip::Stm32 => "stm32",
            Chip::Nrf51 => "nrf51",
        }
    }
}

/// An STM32 or nRF51 image of an upgrade.
struct Image {
    chip: Chip,
    firmware: Firmware,
}

/// The STM32 and nRF51 firmware of an upgrade, STM32 first.
fn images(upgrade: &FirmwareUpgrade) -> Vec<Image> {
    let mut images: Vec<Image> = upgrade
        .get_firmware_for_bootloader()
        .into_iter()
        .filter(|firmware| firmware.file_type == "fw")
        .filter_map(|firmware| {
            let chip = match firmware.target.as_str() {
                "stm32" => Chip::Stm32,
                "nrf51" => Chip::Nrf51,
                _ => return None,
            };
            Some(Image { chip, firmware })
        })
        .collect();
    images.sort_by_key(|image| image.chip != Chip::Stm32);
    images
}

/// Whether the Crazyflie has to be restarted into its bootloader for this
/// upgrade.
fn needs_bootloader(upgrade: &FirmwareUpgrade) -> bool {
    !images(upgrade).is_empty() || upgrade.nrf51_softdevice().is_some()
}

/// The broadcast address of a broadcast group. A Crazyflie that fails is
/// left in its bootloader, still armed, so every group gets its own address:
/// it then can't receive the images of a later group, which are for another
/// platform.
fn broadcast_address(group: usize) -> [u8; 5] {
    let mut address = BROADCAST_ADDRESS;
    address[4] = address[4].wrapping_sub(group as u8);
    address
}

/// What happened to each Crazyflie, by target index.
#[derive(Default)]
struct Outcomes {
    results: Vec<(usize, Result<String>)>,
    /// The bootloader URIs of the Crazyflies sent back into their firmware,
    /// to check that they left.
    restarted: Vec<(usize, String)>,
}

/// A Crazyflie sitting in its bootloader.
struct Session<'j, 'u> {
    job: &'j Job<'u>,
    name: String,
    /// The Crazyradio it is reached with.
    radio: usize,
    loader: CFLoader,
}

impl Session<'_, '_> {
    /// Where its bootloader answers.
    fn bootloader_uri(&mut self) -> String {
        bootloader::bootloader_uri(self.radio, self.loader.bllink_mut().address())
    }

    /// An error that leaves it in its bootloader, saying where that is.
    fn left_in_bootloader(&mut self, e: anyhow::Error) -> anyhow::Error {
        let uri = self.bootloader_uri();
        let address = uri.split('?').next().unwrap_or_default().to_string();
        e.context(format!("left in its bootloader at {}", address))
    }

    /// Note an error for it, leaving it in its bootloader.
    fn fail(&mut self, e: anyhow::Error, outcomes: &mut Outcomes) {
        let e = self.left_in_bootloader(e);
        outcomes.results.push((self.job.index, Err(e)));
    }

    /// Restart it into its firmware and note `result` for it, or the error
    /// if the restart fails.
    async fn restart(&mut self, result: Result<String>, outcomes: &mut Outcomes) {
        match self.loader.reset_to_firmware().await {
            Ok(()) => {
                let uri = self.bootloader_uri();
                outcomes.restarted.push((self.job.index, uri));
                outcomes.results.push((self.job.index, result));
            }
            Err(e) => self.fail(e.context("couldn't restart it into its firmware"), outcomes),
        }
    }

    fn info(&self, chip: Chip) -> &InfoPacket {
        match chip {
            Chip::Stm32 => self.loader.stm32_info(),
            Chip::Nrf51 => self.loader.nrf51_info(),
        }
    }

    /// Both bootloaders take broadcast: the nRF51 receives on the broadcast
    /// address and forwards to the STM32.
    fn broadcast(&self) -> bool {
        self.crc(Chip::Stm32) && self.crc(Chip::Nrf51)
    }

    /// The bootloader of the chip has the CRC commands.
    fn crc(&self, chip: Chip) -> bool {
        self.info(chip).version() >= BROADCAST_PROTOCOL
    }

    fn start(&self, image: &Image) -> u32 {
        bootloader::start_address(&image.firmware, self.info(image.chip))
    }

    /// An image for the start of the firmware area that is bigger than it.
    fn too_big<'i>(&self, images: &'i [Image]) -> Option<&'i Image> {
        images.iter().find(|image| {
            let info = self.info(image.chip);
            let capacity = info.n_flash_page().saturating_sub(info.flash_start()) as usize * info.page_size() as usize;
            image.firmware.start_override.is_none() && image.firmware.data.len() > capacity
        })
    }

    /// What has to be the same for a broadcast chunked for one Crazyflie to
    /// land in the right place on another.
    fn layout(&self) -> [u16; 6] {
        let (nrf51, stm32) = (self.loader.nrf51_info(), self.loader.stm32_info());
        [
            nrf51.page_size(),
            nrf51.n_buff_page(),
            nrf51.flash_start(),
            stm32.page_size(),
            stm32.n_buff_page(),
            stm32.flash_start(),
        ]
    }
}

/// "1 page", "3 pages"
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

/// Flash every job: by broadcast where the bootloaders take it, one at a time
/// otherwise. Returns a result for each job, by target index.
pub async fn flash(runner: &Runner<'_>, jobs: &[Job<'_>], toc_cache: ConfigTocCache) -> Vec<(usize, Result<String>)> {
    let link_context = runner.link_context();
    let mut outcomes = Outcomes::default();

    // Into the bootloader, a few Crazyflies per Crazyradio at a time.
    let to_enter: Vec<&Job> = jobs.iter().filter(|job| needs_bootloader(job.upgrade)).collect();
    let indices: Vec<usize> = to_enter.iter().map(|job| job.index).collect();
    let entered = runner
        .each_of(&indices, "Entering bootloader", BOOTLOADER_PER_RADIO, async |target| {
            if target.radio.is_none() {
                bail!("stm32-fw / nrf51-fw flashing requires a radio URI; USB only supports deck/deck-ctrl targets");
            }
            bootloader::enter_bootloader_for_cfloader(link_context, &target.link_uri).await
        })
        .await;
    let mut in_bootloader = Vec::new();
    for (job, address) in to_enter.into_iter().zip(entered) {
        match address {
            Ok(address) => in_bootloader.push((job, address)),
            Err(e) => outcomes.results.push((job.index, Err(e))),
        }
    }

    let sessions = open_sessions(runner, in_bootloader, &mut outcomes).await;

    // The nRF51 bootloader+softdevice where the release has a newer one; one
    // Crazyflie at a time, since replacing a bootloader isn't broadcast.
    let mut upgraded = Vec::new();
    for mut session in sessions {
        let label = format!("{} nrf51-sd", session.name);
        match bootloader::upgrade_nrf51_softdevice(&mut session.loader, session.job.upgrade, &label).await {
            Ok(()) => upgraded.push(session),
            // It is either restarted into its firmware or left in the
            // bootloader; the error says which.
            Err(e) => outcomes.results.push((session.job.index, Err(e))),
        }
    }

    // Broadcast groups: the Crazyflies of one platform (the same images)
    // whose bootloaders take it and have the same flash layout as the first.
    let mut groups: Vec<Vec<Session>> = Vec::new();
    let mut unicast: Vec<Session> = Vec::new();
    for mut session in upgraded {
        let images = images(session.job.upgrade);
        if let Some(image) = session.too_big(&images) {
            let error = anyhow!(
                "{} doesn't fit in the {} flash ({} bytes)",
                image.firmware.file_name,
                image.chip.name(),
                image.firmware.data.len()
            );
            session.restart(Err(error), &mut outcomes).await;
            continue;
        }
        if images.is_empty() {
            // Only the nRF51 bootloader+softdevice was flashed, or needed.
            session.restart(Ok("flashed".to_string()), &mut outcomes).await;
            continue;
        }
        if !session.broadcast() {
            unicast.push(session);
            continue;
        }
        let group = groups.iter_mut().find(|group| {
            group[0].job.platform == session.job.platform && group[0].layout() == session.layout()
        });
        match group {
            Some(group) => group.push(session),
            None => groups.push(vec![session]),
        }
    }

    for (n, mut group) in groups.into_iter().enumerate() {
        if group.len() < MIN_BROADCAST {
            unicast.append(&mut group);
            continue;
        }
        let rest = broadcast_group(group, broadcast_address(n), &mut outcomes).await;
        unicast.extend(rest);
    }

    unicast.sort_by_key(|session| session.job.index);
    let total = unicast.len();
    for (n, mut session) in unicast.into_iter().enumerate() {
        println!();
        println!(
            "{} (unicast, {} of {}, {}): {}",
            session.name,
            n + 1,
            total,
            session.job.platform,
            runner.targets[session.job.index].uri
        );
        match flash_unicast(&mut session).await {
            Ok(()) => session.restart(Ok("flashed (unicast)".to_string()), &mut outcomes).await,
            Err(e) => session.fail(e, &mut outcomes),
        }
    }

    // Every Crazyflie sent back into its firmware has to have left its
    // bootloader.
    if !outcomes.restarted.is_empty() {
        sleep(Duration::from_millis(500)).await;
    }
    for (index, bootloader_uri) in std::mem::take(&mut outcomes.restarted) {
        if bootloader::left_bootloader(link_context, &bootloader_uri).await {
            continue;
        }
        let address = bootloader_uri.split('?').next().unwrap_or_default();
        if let Some((_, result)) = outcomes.results.iter_mut().find(|(i, _)| *i == index) {
            *result = Err(anyhow!(
                "it is still in its bootloader, at {}. Restart it with its power button",
                address
            ));
        }
    }

    // Decks, once the Crazyflies run their new firmware.
    let decks: Vec<&Job> = jobs
        .iter()
        .filter(|job| {
            !job.upgrade.get_firmware_for_decks().is_empty() || !job.upgrade.get_firmware_for_deckctrl().is_empty()
        })
        .filter(|job| outcomes.results.iter().all(|(index, result)| *index != job.index || result.is_ok()))
        .collect();
    if !decks.is_empty() && jobs.iter().any(|job| needs_bootloader(job.upgrade)) {
        println!();
        println!("Waiting for the Crazyflies to restart...");
        sleep(RESTART_DELAY).await;
    }
    if !decks.is_empty() {
        let names: Vec<&str> = decks.iter().map(|job| runner.targets[job.index].name.as_str()).collect();
        println!();
        println!("Flashing decks: {}", names.join(", "));
    }
    let indices: Vec<usize> = decks.iter().map(|job| job.index).collect();
    let flashed = runner
        .run(indices, DECKS_PER_RADIO, async |i| {
            let job = decks.iter().find(|job| job.index == i).expect("a job for every index");
            let target = &runner.targets[i];
            let output = DeckOutput::Prefixed(&target.name);
            bootloader::flash_decks(link_context, &target.link_uri, toc_cache.clone(), job.upgrade, output).await
        })
        .await;
    for (i, flashed) in flashed {
        match outcomes.results.iter_mut().find(|(index, _)| *index == i) {
            Some((_, result)) => {
                if let Err(e) = flashed {
                    *result = Err(e.context("the STM32 and nRF51 firmware were flashed, the decks failed"));
                }
            }
            None => outcomes.results.push((i, flashed.map(|()| "flashed".to_string()))),
        }
    }

    outcomes.results
}

/// Get the Crazyradio for the bootloaders, and read the bootloader info of
/// every Crazyflie that entered its bootloader. The ones whose bootloader
/// doesn't answer are left in it, with an error saying so.
async fn open_sessions<'j, 'u>(
    runner: &Runner<'_>,
    in_bootloader: Vec<(&'j Job<'u>, [u8; 5])>,
    outcomes: &mut Outcomes,
) -> Vec<Session<'j, 'u>> {
    let Some((first, _)) = in_bootloader.first() else {
        return Vec::new();
    };
    let radio_index = runner.targets[first.index].radio.unwrap_or(0);
    let radio = match bootloader_radio(runner, radio_index).await {
        Ok(radio) => radio,
        Err(e) => {
            let message = format!("{:#}", e.context("in the bootloader, but the Crazyradio couldn't be opened"));
            for (job, _) in in_bootloader {
                outcomes.results.push((job.index, Err(anyhow!(message.clone()))));
            }
            return Vec::new();
        }
    };

    let bar = progress("Reading bootloaders", in_bootloader.len());
    let mut sessions = Vec::new();
    for (job, address) in in_bootloader {
        let opened = async {
            let bllink = Bllink::new_with_radio(radio.clone(), Some(&address)).await?;
            CFLoader::new(bllink).await
        }
        .await;
        match opened {
            Ok(loader) => sessions.push(Session {
                job,
                name: runner.targets[job.index].name.clone(),
                radio: radio_index,
                loader,
            }),
            Err(e) => {
                let address = bootloader::bootloader_uri(radio_index, &address);
                let address = address.split('?').next().unwrap_or_default();
                let e = e.context(format!("its bootloader didn't answer, left in it at {}", address));
                outcomes.results.push((job.index, Err(e)));
            }
        }
        bar.inc(1);
    }
    bar.finish_and_clear();
    sessions
}

/// The Crazyradio, shared with the links of the link context.
async fn bootloader_radio(runner: &Runner<'_>, index: usize) -> Result<SharedCrazyradio> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match runner.link_context().get_radio(index).await {
            Ok(radio) => return Ok(radio),
            Err(e) if attempt >= RADIO_ATTEMPTS => return Err(anyhow!("Crazyradio {}: {}", index, e)),
            Err(_) => sleep(RADIO_RETRY).await,
        }
    }
}

/// Broadcast each image to the group, then check every Crazyflie in it by
/// CRC, flash what is missing again and restart it into its firmware.
/// Returns the Crazyflies that couldn't be armed, to be flashed over unicast.
async fn broadcast_group<'j, 'u>(
    group: Vec<Session<'j, 'u>>,
    address: [u8; 5],
    outcomes: &mut Outcomes,
) -> Vec<Session<'j, 'u>> {
    let mut armed = Vec::new();
    let mut rest = Vec::new();
    for mut session in group {
        match session.loader.set_broadcast_address(&address).await {
            Ok(()) => armed.push(session),
            Err(e) => {
                eprintln!("{}: couldn't arm it for broadcast, flashing it over unicast: {:#}", session.name, e);
                rest.push(session);
            }
        }
    }
    if armed.len() < MIN_BROADCAST {
        rest.append(&mut armed);
        return rest;
    }

    let images = images(armed[0].job.upgrade);
    let names: Vec<&str> = armed.iter().map(|session| session.name.as_str()).collect();
    println!();
    println!(
        "Broadcast to {} ({}): {}",
        count(armed.len(), "Crazyflie"),
        armed[0].job.platform,
        names.join(", ")
    );

    for image in &images {
        let start = armed[0].start(image);
        let label = format!("{} → {} CFs", image.chip.label(), armed.len());
        let bar = get_progressbar(image.firmware.data.len(), Some(&label));
        let pb = bar.clone();
        let callback = move |written: usize, _total: usize| pb.set_position(written as u64);
        let sent = armed[0]
            .loader
            .broadcast_flash_image_with_progress(image.chip.target(), start, &image.firmware.data, Some(callback))
            .await;
        if let Err(e) = sent {
            bar.abandon();
            let message = format!("{:#}", e.context(format!("broadcasting the {} firmware failed", image.chip.name())));
            for session in &mut armed {
                session.fail(anyhow!(message.clone()), outcomes);
            }
            return rest;
        }
        finish_progress(&bar, format!("{} firmware sent to {}", image.chip.name(), count(armed.len(), "Crazyflie")));
    }

    let bar = progress("Verifying", armed.len());
    for mut session in armed {
        let verified = async {
            let mut fixed = 0;
            for image in &images {
                let start = session.start(image);
                fixed += verify_and_fix(&mut session.loader, image, start).await?;
            }
            Ok::<_, anyhow::Error>(fixed)
        }
        .await;
        match verified {
            Ok(fixed) => {
                let done = if fixed == 0 {
                    "flashed (broadcast)".to_string()
                } else {
                    format!("flashed (broadcast, {} flashed again)", count(fixed, "page"))
                };
                session.restart(Ok(done), outcomes).await;
            }
            Err(e) => session.fail(e, outcomes),
        }
        bar.inc(1);
    }
    bar.finish_and_clear();
    rest
}

/// Compare an image with the flash by CRC and flash the pages that differ
/// again, up to [`FIX_ATTEMPTS`] times. Returns how many pages were flashed
/// again.
async fn verify_and_fix(loader: &mut CFLoader, image: &Image, start: u32) -> Result<usize> {
    let (target, data) = (image.chip.target(), &image.firmware.data);
    let verifying = || format!("checking the {} firmware failed", image.chip.name());
    let mut failed = loader.verify_flash_crc(target, start, data).await.with_context(verifying)?;
    let mut fixed = 0;
    for _ in 0..FIX_ATTEMPTS {
        if failed.is_empty() {
            return Ok(fixed);
        }
        loader
            .fix_failed_pages(target, start, data, &failed)
            .await
            .with_context(|| format!("flashing {} of the {} firmware again failed", count(failed.len(), "page"), image.chip.name()))?;
        fixed += failed.len();
        failed = loader.verify_flash_crc(target, start, data).await.with_context(verifying)?;
    }
    if !failed.is_empty() {
        bail!(
            "{} of the {} firmware still differ after flashing them again {} times",
            count(failed.len(), "page"),
            image.chip.name(),
            FIX_ATTEMPTS
        );
    }
    Ok(fixed)
}

/// Flash the images to one Crazyflie over unicast, and check them by CRC
/// where the bootloader can.
async fn flash_unicast(session: &mut Session<'_, '_>) -> Result<()> {
    for image in images(session.job.upgrade) {
        let start = session.start(&image);
        let bar = get_progressbar(image.firmware.data.len(), Some(image.chip.label()));
        let pb = bar.clone();
        let callback = move |written: usize, _total: usize| pb.set_position(written as u64);
        session
            .loader
            .flash_image_with_progress(image.chip.target(), start, &image.firmware.data, Some(callback))
            .await
            .with_context(|| format!("flashing the {} firmware failed", image.chip.name()))?;
        finish_progress(&bar, format!("{} flashed", image.chip.name()));
        if session.crc(image.chip) {
            let fixed = verify_and_fix(&mut session.loader, &image, start).await?;
            if fixed > 0 {
                println!("{} flashed again", count(fixed, "page"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_has_its_own_broadcast_address() {
        assert_eq!(broadcast_address(0), BROADCAST_ADDRESS);
        assert_eq!(broadcast_address(1), [0xBC, 0xE7, 0xE7, 0xE7, 0xE6]);
        assert_ne!(broadcast_address(1), broadcast_address(2));
    }
}
