use crate::error::CliError;
use crate::utils::display::csv_escape;
use anyhow::{anyhow, bail, Result};
use colored::*;
use crazyflie_lib::subsystems::console::{ConsoleError, ConsoleHandle};
use futures::StreamExt;
use inquire::{InquireError, Select};
use std::io::Write;

pub fn format_console_line(line: &str) -> String {
    if let Some((subsystem, message)) = line.split_once(':') {
        format!("{}:{}", subsystem.bold(), message)
    } else {
        line.to_string()
    }
}

/// Path of the Crazyflie's STM32 console, which every firmware version has.
/// Only cfcli's own preserved history file and its internal uses are tied to
/// this console: to the user it is one choice among the others.
pub const STM32_CONSOLE: &str = "cf:stm32";

pub async fn list(cf: &crazyflie_lib::Crazyflie, csv: bool) -> Result<()> {
    let paths = cf.console.list().await?;
    print!("{}", render_paths(&paths, csv));

    Ok(())
}

fn render_paths(paths: &[String], csv: bool) -> String {
    if csv {
        let mut output = "path\n".to_string();
        for path in paths {
            output.push_str(&csv_escape(path));
            output.push('\n');
        }
        output
    } else {
        format!("Path\n----\n{}\n", paths.join("\n"))
    }
}

fn console_not_found(path: &str, available: &[String]) -> CliError {
    CliError::NotFound(format!(
        "console '{}'; available consoles: {}",
        path,
        available.join(", ")
    ))
}

/// Ask which console to show. There is nothing to ask when the Crazyflie only
/// has one, which is always the case with firmware older than protocol 13.
pub async fn pick(cf: &crazyflie_lib::Crazyflie) -> Result<String> {
    let mut paths = cf.console.list().await?;
    if paths.len() == 1 {
        return Ok(paths.remove(0));
    }

    // The prompt blocks on the terminal: keep the link's tasks running meanwhile.
    tokio::task::block_in_place(|| Select::new("Select a console:", paths).prompt()).map_err(
        |err| match err {
            InquireError::OperationInterrupted => CliError::Interrupted.into(),
            _ => anyhow!("No console selected"),
        },
    )
}

/// Replay and then follow one console. The handle is stored in `open_console`
/// so the caller can close it when this future is cancelled by a timeout or a
/// signal.
pub async fn print(
    cf: &crazyflie_lib::Crazyflie,
    path: &str,
    no_format: bool,
    open_console: &mut Option<ConsoleHandle>,
) -> Result<()> {
    let console = match cf.console.open(path).await {
        Ok(console) => open_console.insert(console),
        Err(crazyflie_lib::Error::Console(ConsoleError::UnknownSource(_))) => {
            bail!(console_not_found(path, &cf.console.list().await?))
        }
        Err(err) => return Err(err.into()),
    };

    let mut stdout = std::io::stdout();
    if no_format {
        let mut text = console.text();
        while let Some(chunk) = text.next().await {
            write!(stdout, "{}", chunk)?;
            stdout.flush()?;
        }
    } else {
        let mut lines = console.lines();
        while let Some(line) = lines.next().await {
            writeln!(stdout, "{}", format_console_line(&line))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<String> {
        vec!["cf:stm32".to_string(), "deck:bcCam".to_string()]
    }

    #[test]
    fn renders_paths_for_humans() {
        assert_eq!(
            render_paths(&paths(), false),
            "Path\n----\ncf:stm32\ndeck:bcCam\n"
        );
    }

    #[test]
    fn renders_paths_as_csv() {
        assert_eq!(
            render_paths(&["cf:stm32".to_string(), "deck,other".to_string()], true),
            "path\ncf:stm32\n\"deck,other\"\n"
        );
    }

    #[test]
    fn unknown_console_reports_the_available_paths() {
        let err = console_not_found("deck:missing", &paths());

        assert_eq!(err.exit_code(), 20);
        assert_eq!(
            err.to_string(),
            "not found: console 'deck:missing'; available consoles: cf:stm32, deck:bcCam"
        );
    }
}
