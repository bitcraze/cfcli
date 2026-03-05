use std::pin::Pin;
use std::future::Future;
use std::io::Write;

use anyhow::{bail, Result};
use crazyflie_lib::NoTocCache;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout, Duration};

use crate::ConfigTocCache;
use crate::modules::bootloader;

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
