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

pub async fn print(cf: &crazyflie_lib::Crazyflie, no_format: bool) -> Result<()> {
            let mut console_stream = cf.console.stream().await;

    while let Some(line) = console_stream.next().await {
        if no_format {
            print!("{}", line);
        } else {
            print!("{}", format_console_line(&line));
        }
    }

    Ok(())
}