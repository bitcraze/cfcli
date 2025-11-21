use std::process::exit;
use std::os::unix::process;
use std::pin::Pin;
use std::future::Future;

use crazyflie_lib::{NoTocCache, subsystems::log::LogPeriod};
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;

// Keep the trait without async_trait (cleaner!)
pub trait StabilityTest {
    fn name(&self) -> &str;
    
    fn run<'a>(
        &'a self,
        link_context: &'a crazyflie_link::LinkContext,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error>>> + Send + 'a>>;
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
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error>>> + Send + 'a>> {
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
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error>>> + Send + 'a>> {
        Box::pin(async move {
            let cf = crazyflie_lib::Crazyflie::connect_from_uri(link_context, uri, NoTocCache).await?;
            let test_value = 1;
            let old_value: u8 = cf.param.get("usd.logging").await?;
            cf.param.set("usd.logging", test_value).await?;
            let new_value: u8 = cf.param.get("usd.logging").await?;
            
            // TODO: Does this make any sense, since we cache it here there's no
            // real readback from the Crazyflie
            if new_value != test_value {
                return Err("Param read/write mismatch".into());
            }

            cf.param.set("usd.logging", old_value).await?;

            let reset_value: u8 = cf.param.get("usd.logging").await?;
            
            if reset_value != old_value {
                return Err("Param reset mismatch".into());
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
    ) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error>>> + Send + 'a>> {
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
) -> Result<(), Box<dyn std::error::Error>> {
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
) -> Result<(), Box<dyn std::error::Error>> {
  let num_tests = tests.len();
  let target_per_test = iterations / num_tests as u32;
  let multi = indicatif::MultiProgress::new();
  let bars: Vec<ProgressBar> = tests
    .iter()
    .map(|test| {
      let bar = multi.add(ProgressBar::new(target_per_test as u64));
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
  
  let mut rng = rand::thread_rng();
  
  let mut test_counts = vec![0u32; num_tests];
  
  while test_counts.iter().any(|&count| count < target_per_test) {
    let available_tests: Vec<usize> = test_counts
      .iter()
      .enumerate()
      .filter(|(_, &count)| count < target_per_test)
      .map(|(idx, _)| idx)
      .collect();
    
    let test_idx = available_tests[rng.gen_range(0..available_tests.len())];

    match tests[test_idx].run(link_context, uri).await {
        Ok(_) => {
            test_counts[test_idx] += 1;
            bars[test_idx].inc(1);
        }
        Err(e) => {
            eprintln!("Error running test {}: {}", tests[test_idx].name(), e);
            exit(1);
        }
    }
  }
  
  for bar in bars {
    bar.finish();
  }
  
  Ok(())
}
