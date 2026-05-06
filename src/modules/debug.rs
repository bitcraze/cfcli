use anyhow::Result;
use crazyflie_lib::Crazyflie;
use futures::StreamExt;
use std::time::Duration;
use tokio::time::timeout;

const ASSERT_INFO_PARAM: &str = "system.assertInfo";

/// Strip a leading `WORD:` (or `WORD: `) subsystem prefix added by the
/// firmware's `DEBUG_PRINT`. Returns the original line if no prefix is
/// present.
fn strip_subsystem_prefix(line: &str) -> &str {
    match line.split_once(':') {
        Some((_, rest)) => rest.strip_prefix(' ').unwrap_or(rest),
        None => line,
    }
}

/// Trigger a firmware assert-info dump and print the resulting line.
/// `printAssertSnapshotData()` emits exactly one `DEBUG_PRINT` line per
/// invocation, so we wait for the first complete line and return. If
/// nothing arrives within `wait_timeout` the firmware never responded —
/// fall back to a local "No assert info" message.
pub async fn assert_dump(cf: &Crazyflie, wait_timeout: Duration) -> Result<()> {
    let mut stream = cf.console.stream().await;

    // Drain any console history / pre-existing output so we don't mix it
    // with the assert dump. The first item from `stream()` is the buffered
    // history; anything that arrives within the drain window is also
    // pre-trigger noise.
    let drain_window = Duration::from_millis(200);
    while timeout(drain_window, stream.next()).await.is_ok() {}

    cf.param.set(ASSERT_INFO_PARAM, 1u8).await?;

    let deadline = tokio::time::Instant::now() + wait_timeout;
    let mut buf = String::new();

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, stream.next()).await {
            Ok(Some(chunk)) => {
                buf.push_str(&chunk);
                if let Some(nl) = buf.find('\n') {
                    let line = &buf[..=nl];
                    print!("{}", strip_subsystem_prefix(line));
                    return Ok(());
                }
            }
            _ => break,
        }
    }

    println!("No assert info");
    Ok(())
}
