use anyhow::Result;
use crazyflie_connection_server::Crazyflie;
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

/// Trigger a firmware assert-info dump and print the resulting line, or "No
/// assert info" when the firmware doesn't answer.
pub async fn assert_dump(cf: &Crazyflie, wait_timeout: Duration) -> Result<()> {
    match assert_info(cf, wait_timeout).await? {
        Some(line) => println!("{}", line),
        None => println!("No assert info"),
    }
    Ok(())
}

/// Trigger a firmware assert-info dump and return the resulting line.
/// `printAssertSnapshotData()` emits exactly one `DEBUG_PRINT` line per
/// invocation, so we wait for the first complete line and return. If
/// nothing arrives within `wait_timeout` the firmware never responded and
/// there is no line.
pub async fn assert_info(cf: &Crazyflie, wait_timeout: Duration) -> Result<Option<String>> {
    // Only the lines printed from now on, so the dump isn't mixed with older
    // output. Anything that arrives within the drain window is pre-trigger
    // noise too.
    let mut lines = cf.console.line_stream_no_history().await?;
    let drain_window = Duration::from_millis(200);
    while timeout(drain_window, lines.next()).await.is_ok_and(|line| line.is_some()) {}

    cf.param.set_lossy(ASSERT_INFO_PARAM, 1.0).await?;

    match timeout(wait_timeout, lines.next()).await {
        Ok(Some(line)) => Ok(Some(strip_subsystem_prefix(&line).trim_end().to_string())),
        _ => Ok(None),
    }
}
