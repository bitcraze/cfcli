use anyhow::Result;
use futures::StreamExt;
use colored::*;

pub fn format_console_line(line: &str) -> String {
    if let Some((subsystem, message)) = line.split_once(':') {
        format!("{}:{}", subsystem.bold(), message)
    } else {
        line.to_string()
    }
}

/// Print the console, starting with what it printed since it was connected.
pub async fn print(cf: &crazyflie_connection_server::Crazyflie, no_format: bool) -> Result<()> {
    let mut lines = cf.console.line_stream().await?;

    while let Some(line) = lines.next().await {
        if no_format {
            println!("{}", line);
        } else {
            println!("{}", format_console_line(&line));
        }
    }

    Ok(())
}