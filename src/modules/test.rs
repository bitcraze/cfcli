use std::pin::Pin;
use std::future::Future;
use std::io::Write;

use anyhow::{bail, Result};
use crazyflie_lib::subsystems::memory::{MemoryType, RawMemory};
use crazyflie_lib::{Crazyflie, NoTocCache};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::IsTerminal;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout, Duration};

use crate::ConfigTocCache;
use crate::modules::bootloader;
use crate::utils::display;

// Keep the trait without async_trait (cleaner!)
pub trait StabilityTest {
    fn name(&self) -> &str;
    
    fn run<'a>(
        &'a self,
        link_context: &'a crazyflie_link::LinkContext,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

pub struct ReconnectTest;

// Clean implementations without async_trait macro
impl StabilityTest for ReconnectTest {
    fn name(&self) -> &str {
        "Reconnection w/o cache"
    }
    
    fn run<'a>(
        &'a self,
        link_context: &'a crazyflie_link::LinkContext,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, NoTocCache).await?;
            cf.disconnect().await;
            Ok(())
        })
    }
}

pub struct ParamReadReadWriteTest;

// Clean implementations without async_trait macro
impl StabilityTest for ParamReadReadWriteTest {
    fn name(&self) -> &str {
        "Set/get param\t"
    }
    
    fn run<'a>(
        &'a self,
        link_context: &'a crazyflie_link::LinkContext,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, NoTocCache).await?;
            let test_value = 1;
            let old_value: u8 = cf.param.get("usd.logging").await?;
            cf.param.set("usd.logging", test_value).await?;
            let new_value: u8 = cf.param.get("usd.logging").await?;
            
            // TODO: Does this make any sense, since we cache it here there's no
            // real readback from the Crazyflie
            if new_value != test_value {
                bail!("Param read/write mismatch");
            }

            cf.param.set("usd.logging", old_value).await?;

            let reset_value: u8 = cf.param.get("usd.logging").await?;
            
            if reset_value != old_value {
                bail!("Param reset mismatch");
            }

            cf.disconnect().await;
            Ok(())
        })
    }
}

pub struct LoggingTest;

// Clean implementations without async_trait macro
impl StabilityTest for LoggingTest {
    fn name(&self) -> &str {
        "Logging\t\t"
    }
    
    fn run<'a>(
        &'a self,
        link_context: &'a crazyflie_link::LinkContext,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, NoTocCache).await?;
            
            let mut block = cf.log.create_block().await?;

            let names = "stateEstimate.x,stateEstimate.y,stateEstimate.z";
            let period = crazyflie_lib::subsystems::log::LogPeriod::from_millis(
                100
            )?;

            for name in names.split(",") {
                block.add_variable(name).await?;
            }

            let stream = block
                .start(period)
                .await?;

            for _ in 0..10 {
              if let Ok(_data) = stream.next().await {
                
              } else {
                break;
              }
            }

            cf.disconnect().await;
            Ok(())
        })
    }
}

pub async fn stability(
    link_context: &crazyflie_link::LinkContext,
    uri: &str,
    iterations: u32,
) -> Result<()> {
    let tests: Vec<Box<dyn StabilityTest>> = vec![
        Box::new(ReconnectTest),
        Box::new(ParamReadReadWriteTest),
        Box::new(LoggingTest),
    ];

    run_stability_tests(link_context, uri, iterations, tests).await
}

async fn run_stability_tests(
    link_context: &crazyflie_link::LinkContext,
    uri: &str,
    iterations: u32,
    tests: Vec<Box<dyn StabilityTest>>,
) -> Result<()> {
  let num_tests = tests.len();
  let multi = indicatif::MultiProgress::new();
  if !std::io::stderr().is_terminal() {
    multi.set_draw_target(ProgressDrawTarget::hidden());
  }
  let bars: Vec<ProgressBar> = tests
    .iter()
    .map(|test| {
      let bar = multi.add(ProgressBar::new(iterations as u64));
      bar.set_style(
        ProgressStyle::default_bar()
          .template(&format!("{} [{{bar:40}}] {{pos}}/{{len}} ({{eta}})", test.name()))
          .unwrap()
          .progress_chars("##-"),
      );
      bar.tick();
      bar
    })
    .collect();
  
  let mut rng = rand::rng();
  
  let mut test_counts = vec![0u32; num_tests];
  
  while test_counts.iter().any(|&count| count < iterations) {
    let available_tests: Vec<usize> = test_counts
      .iter()
      .enumerate()
      .filter(|(_, &count)| count < iterations)
      .map(|(idx, _)| idx)
      .collect();
    
    let test_idx = available_tests[rng.random_range(0..available_tests.len())];

    match tests[test_idx].run(link_context, uri).await {
        Ok(_) => {
            test_counts[test_idx] += 1;
            bars[test_idx].inc(1);
        }
        Err(e) => {
            return Err(e.into());
        }
    }
  }
  
  for bar in bars {
    bar.finish();
  }

  Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct RebootTestResult {
    iteration: u32,
    selftest_passed: bool,
    console: String,
}

pub async fn reboot(
    link_context: &crazyflie_link::LinkContext,
    uri: &str,
    toc_cache: ConfigTocCache,
    iterations: u32,
) -> Result<()> {
    use colored::Colorize;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let output_file = format!("reboot_test_results_{}.json", timestamp);
    let output_file = output_file.as_str();
    let mut results: Vec<RebootTestResult> = Vec::new();
    let mut reboot_time: Option<std::time::Instant> = None;
    let mut fail_count: u32 = 0;

    let term_width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
    let label = "Reboot test";
    let bar_width = term_width.saturating_sub(50 + label.len());

    let bar = ProgressBar::new(iterations as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template(&format!("{} [{{elapsed_precise}}] [{{bar:{}.cyan/blue}}] {{pos}}/{{len}} {{msg}}", label, bar_width))
            .unwrap()
            .progress_chars("#>-"),
    );
    if !std::io::stderr().is_terminal() {
        bar.set_draw_target(ProgressDrawTarget::hidden());
    }
    bar.enable_steady_tick(Duration::from_millis(100));

    // Start by rebooting
    bar.set_message("rebooting...".to_string());
    bootloader::reboot(link_context, uri).await?;

    for i in 1..=iterations {
        bar.set_message(format!("connecting..."));

        let connect_deadline = std::time::Instant::now() + Duration::from_secs(7);
        let cf = loop {
            let remaining = connect_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break None;
            }
            match timeout(remaining, crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, toc_cache.clone())).await {
                Ok(Ok(cf)) => break Some(cf),
                Ok(Err(_)) | Err(_) => {
                    sleep(Duration::from_millis(500)).await;
                }
            }
        };

        let (passed, console_lines) = if let Some(cf) = cf {
            bar.set_message("reading selftest...".to_string());

            // Read system.selftestPassed
            let selftest_passed: i8 = cf.param.get("system.selftestPassed").await?;
            let passed = selftest_passed != 0;

            // Collect console lines until it's time to reboot (10s since last reboot)
            bar.set_message("collecting console...".to_string());
            let mut console_lines: Vec<String> = Vec::new();
            let mut line_stream = cf.console.line_stream().await;

            let console_deadline = if i < iterations {
                let since_reboot = reboot_time.map_or(Duration::ZERO, |rt| rt.elapsed());
                let min_interval = Duration::from_secs(10);
                if since_reboot < min_interval {
                    min_interval - since_reboot
                } else {
                    Duration::from_millis(500)
                }
            } else {
                Duration::from_millis(500)
            };
            let deadline = std::time::Instant::now() + console_deadline;

            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match timeout(remaining, line_stream.next()).await {
                    Ok(Some(line)) => {
                        console_lines.push(line);
                    }
                    _ => {
                        break;
                    }
                }
            }

            cf.disconnect().await;
            (passed, console_lines.join("\n"))
        } else {
            bar.println(format!("{}", format!("  Iteration {}: connect timeout", i).red()));
            (false, String::new())
        };

        if !passed {
            fail_count += 1;
        }

        // Save result for this iteration
        let result = RebootTestResult {
            iteration: i,
            selftest_passed: passed,
            console: console_lines,
        };
        results.push(result);

        // Write results to file after each iteration
        let json = serde_json::to_string_pretty(&results)?;
        let mut file = std::fs::File::create(output_file)?;
        file.write_all(json.as_bytes())?;

        bar.inc(1);

        if fail_count > 0 {
            bar.set_message(format!("{}", format!("{} failed", fail_count).red()));
        } else {
            bar.set_message(String::new());
        }

        if i < iterations {
            bar.set_message(if fail_count > 0 {
                format!("{} rebooting...", format!("{} failed", fail_count).red())
            } else {
                "rebooting...".to_string()
            });
            bootloader::reboot(link_context, uri).await?;
            reboot_time = Some(std::time::Instant::now());
        }
    }

    let pass_count = results.iter().filter(|r| r.selftest_passed).count();
    let summary = if fail_count > 0 {
        format!("{}/{} passed ({})", pass_count, iterations, format!("{} failed", fail_count).red())
    } else {
        format!("{}/{} passed", pass_count, iterations)
    };
    bar.finish_with_message(summary);
    println!("Results saved to {}", output_file);

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPerfTest {
    All,
    Ping,
    Uplink,
    Downlink,
    Echo,
}

/// CRTP max data payload — used to derive packets/sec from bytes/sec for the
/// uplink/downlink tests (sink and source both use full-size payloads).
const CRTP_MAX_PAYLOAD: f64 = 30.0;

fn fmt_bw(bytes_per_sec: f64) -> String {
    let kbit = bytes_per_sec * 8.0 / 1000.0;
    let pkt = bytes_per_sec / CRTP_MAX_PAYLOAD;
    format!("{:>7.1} kbit/s  {:>6.0} B/s  {:>6.1} pkt/s", kbit, bytes_per_sec, pkt)
}

/// Emit one bandwidth result as either an aligned human row or three CSV rows
/// (`<prefix>_kbit_per_sec`, `<prefix>_bytes_per_sec`, `<prefix>_packets_per_sec`).
fn emit_bandwidth(label: &str, csv_prefix: &str, n_packets: u64, bps: f64, csv: bool) {
    let kbit = bps * 8.0 / 1000.0;
    let pkt = bps / CRTP_MAX_PAYLOAD;
    if csv {
        display::csv_row(&[&format!("{}_kbit_per_sec", csv_prefix), &format!("{:.3}", kbit), "kbit/s"]);
        display::csv_row(&[&format!("{}_bytes_per_sec", csv_prefix), &format!("{:.3}", bps), "B/s"]);
        display::csv_row(&[&format!("{}_packets_per_sec", csv_prefix), &format!("{:.3}", pkt), "pkt/s"]);
    } else {
        let _ = n_packets;
        println!("  {} {}", label, fmt_bw(bps));
    }
}

pub async fn link_perf(
    cf: &Crazyflie,
    test: LinkPerfTest,
    n_packets: u64,
    n_pings: u32,
    csv: bool,
) -> Result<()> {
    let run_ping = matches!(test, LinkPerfTest::All | LinkPerfTest::Ping);
    let run_up = matches!(test, LinkPerfTest::All | LinkPerfTest::Uplink);
    let run_down = matches!(test, LinkPerfTest::All | LinkPerfTest::Downlink);
    let run_echo = matches!(test, LinkPerfTest::All | LinkPerfTest::Echo);

    if csv {
        display::csv_row(&["metric", "value", "unit"]);
    } else {
        println!("Link performance benchmark (CRTP link service, port 15)");
        println!();
    }

    if run_ping {
        if n_pings == 0 {
            bail!("ping count must be > 0");
        }
        let mut samples = Vec::with_capacity(n_pings as usize);
        for _ in 0..n_pings {
            samples.push(cf.link_service.ping().await?);
        }
        let min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let avg = samples.iter().sum::<f64>() / samples.len() as f64;
        if csv {
            display::csv_row(&["ping_samples", &n_pings.to_string(), "count"]);
            display::csv_row(&["ping_min_ms", &format!("{:.3}", min), "ms"]);
            display::csv_row(&["ping_avg_ms", &format!("{:.3}", avg), "ms"]);
            display::csv_row(&["ping_max_ms", &format!("{:.3}", max), "ms"]);
        } else {
            println!(
                "  Latency   (ping, n={})    min {:.2}  avg {:.2}  max {:.2}  ms",
                n_pings, min, avg, max
            );
        }
    }

    if run_up {
        let bps = cf.link_service.test_uplink_bandwidth(n_packets).await?;
        emit_bandwidth(&format!("Uplink    (sink, n={})  ", n_packets), "uplink", n_packets, bps, csv);
    }

    if run_down {
        let bps = cf.link_service.test_downlink_bandwidth(n_packets).await?;
        emit_bandwidth(&format!("Downlink  (source, n={})", n_packets), "downlink", n_packets, bps, csv);
    }

    if run_echo {
        let res = cf.link_service.test_echo_bandwidth(n_packets).await?;
        emit_bandwidth(&format!("Echo      (echo, n={})  ", n_packets), "echo", n_packets, res.uplink_bytes_per_sec, csv);
    }

    let stats = cf.link_service.get_statistics().await;
    if stats.link_quality.is_some() {
        if csv {
            if let Some(lq) = stats.link_quality {
                display::csv_row(&["link_quality", &format!("{:.4}", lq), "ratio"]);
            }
            if let Some(rssi) = stats.rssi {
                display::csv_row(&["rssi_dbm", &format!("{:.2}", rssi), "dBm"]);
            }
            if let Some(r) = stats.uplink_rate {
                display::csv_row(&["radio_uplink_rate", &format!("{:.3}", r), "pkt/s"]);
            }
            if let Some(r) = stats.downlink_rate {
                display::csv_row(&["radio_downlink_rate", &format!("{:.3}", r), "pkt/s"]);
            }
            if let Some(r) = stats.radio_send_rate {
                display::csv_row(&["radio_send_rate", &format!("{:.3}", r), "pkt/s"]);
            }
            if let Some(r) = stats.avg_retries {
                display::csv_row(&["radio_avg_retries", &format!("{:.4}", r), "retries"]);
            }
            if let Some(r) = stats.power_detector_rate {
                display::csv_row(&["radio_power_detector_rate", &format!("{:.4}", r), "ratio"]);
            }
        } else {
            println!();
            println!("Radio link statistics:");
            if let Some(lq) = stats.link_quality {
                println!("  link quality    {:>6.2} %", lq * 100.0);
            }
            if let Some(rssi) = stats.rssi {
                println!("  rssi            {:>6.1} dBm", rssi);
            }
            if let Some(r) = stats.uplink_rate {
                println!("  uplink rate     {:>6.1} pkt/s", r);
            }
            if let Some(r) = stats.downlink_rate {
                println!("  downlink rate   {:>6.1} pkt/s", r);
            }
            if let Some(r) = stats.radio_send_rate {
                println!("  radio send      {:>6.1} pkt/s", r);
            }
            if let Some(r) = stats.avg_retries {
                println!("  avg retries     {:>6.2}", r);
            }
            if let Some(r) = stats.power_detector_rate {
                println!("  power detector  {:>6.2} %", r * 100.0);
            }
        }
    }

    Ok(())
}

/// One-shot read of a single u32 log variable via a temporary 100 ms block.
async fn read_log_u32(cf: &Crazyflie, name: &str) -> Result<u32> {
    let mut block = cf.log.create_block().await?;
    block.add_variable(name).await?;
    let stream = block
        .start(crazyflie_lib::subsystems::log::LogPeriod::from_millis(100)?)
        .await?;
    let data = match timeout(Duration::from_secs(2), stream.next()).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => bail!("log read for {} failed: {}", name, e),
        Err(_) => bail!("log read for {} timed out", name),
    };
    let value = *data
        .data
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("log variable {} not in sample", name))?;
    Ok(value.try_into()?)
}

/// Memory performance test against the firmware MemoryTester.
///
/// The firmware tester returns/expects byte `(addr & 0xff)` at every address.
/// We write that pattern, read it back, verify both directions, and report
/// throughput. The firmware-side write error counter (`memTst.errCntW`) is
/// also checked to confirm the firmware accepted every byte.
pub async fn mem_perf(
    cf: &Crazyflie,
    length: usize,
    csv: bool,
) -> Result<()> {
    if length == 0 {
        bail!("length must be > 0");
    }

    let memories = cf.memory.get_memories(Some(MemoryType::MemoryTester));
    if memories.len() != 1 {
        bail!("expected exactly one MemoryTester, found {}", memories.len());
    }
    let device = memories[0].clone();
    let mem_size = device.size as usize;
    if length > mem_size {
        bail!("requested length {} exceeds MemoryTester size {}", length, mem_size);
    }

    // Reset the firmware-side write verification error counter
    cf.param.set("memTst.resetW", 1u8).await?;

    let raw = match cf.memory.open_memory::<RawMemory>(device).await {
        Some(Ok(m)) => m,
        Some(Err(e)) => bail!("Could not open MemoryTester: {}", e),
        None => bail!("MemoryTester not found"),
    };

    let pattern: Vec<u8> = (0..length).map(|i| (i & 0xff) as u8).collect();

    if !csv {
        println!("Memory tester performance ({} bytes)", length);
        println!();
    }

    let write_start = std::time::Instant::now();
    raw.write(0, &pattern).await?;
    let write_secs = write_start.elapsed().as_secs_f64();

    let read_start = std::time::Instant::now();
    let read_back = raw.read(0, length).await?;
    let read_secs = read_start.elapsed().as_secs_f64();

    if read_back != pattern {
        let mismatch = read_back
            .iter()
            .zip(pattern.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        bail!(
            "read-back mismatch at offset {}: expected 0x{:02x}, got 0x{:02x}",
            mismatch, pattern[mismatch], read_back[mismatch]
        );
    }

    let fw_err_count: u32 = read_log_u32(cf, "memTst.errCntW").await?;

    let write_bps = length as f64 / write_secs;
    let read_bps = length as f64 / read_secs;

    if csv {
        display::csv_row(&["metric", "value", "unit"]);
        display::csv_row(&["mem_perf_bytes", &length.to_string(), "B"]);
        display::csv_row(&["mem_perf_write_seconds", &format!("{:.3}", write_secs), "s"]);
        display::csv_row(&["mem_perf_read_seconds", &format!("{:.3}", read_secs), "s"]);
        emit_bandwidth("", "mem_perf_write", 0, write_bps, true);
        emit_bandwidth("", "mem_perf_read", 0, read_bps, true);
        display::csv_row(&["mem_perf_fw_write_errors", &fw_err_count.to_string(), "count"]);
    } else {
        println!("  Write {:>6} B in {:>5.2} s   {}", length, write_secs, fmt_bw(write_bps));
        println!("  Read  {:>6} B in {:>5.2} s   {}", length, read_secs, fmt_bw(read_bps));
        println!();
        println!("  Read-back verified ({} bytes match expected pattern)", length);
        if fw_err_count == 0 {
            println!("  Firmware write errors: 0");
        } else {
            println!("  Firmware write errors: {} (memTst.errCntW)", fw_err_count);
        }
    }

    if fw_err_count != 0 {
        bail!("firmware reported {} write verification errors", fw_err_count);
    }

    Ok(())
}
