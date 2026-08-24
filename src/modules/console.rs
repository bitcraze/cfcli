use crate::error::CliError;
use crate::utils::display::csv_escape;
use anyhow::{bail, Result};
use colored::*;
use crazyflie_lib::subsystems::console::{ConsoleCatalog, ConsoleHistory, ConsoleSourceSelector};
use futures::StreamExt;
use std::io::Write;

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

pub async fn list_sources(cf: &crazyflie_lib::Crazyflie, csv: bool) -> Result<()> {
    let catalog = cf.console.catalog().await?;
    let paths = catalog
        .iter()
        .map(|source| source.path())
        .collect::<Vec<_>>();
    print!("{}", render_source_paths(&paths, csv));

    Ok(())
}

fn render_source_paths(paths: &[&str], csv: bool) -> String {
    if csv {
        let mut output = "path\n".to_string();
        for path in paths {
            output.push_str(&csv_escape(path));
            output.push('\n');
        }
        output
    } else if paths.is_empty() {
        "No sourced console sources found.\n".to_string()
    } else {
        format!("Path\n----\n{}\n", paths.join("\n"))
    }
}

fn source_not_found(path: &str, catalog: &ConsoleCatalog) -> CliError {
    let available = catalog
        .iter()
        .map(|source| source.path())
        .collect::<Vec<_>>();
    let suffix = if available.is_empty() {
        "no sourced console sources are available".to_string()
    } else {
        format!("available sources: {}", available.join(", "))
    };

    CliError::NotFound(format!("console source '{}'; {}", path, suffix))
}

pub async fn print_source(
    cf: &crazyflie_lib::Crazyflie,
    path: &str,
    no_format: bool,
    enabled_source: &mut Option<ConsoleSourceSelector>,
) -> Result<()> {
    let catalog = cf.console.catalog().await?;
    let source = match catalog.find(path) {
        Some(source) => source.clone(),
        None => bail!(source_not_found(path, &catalog)),
    };
    let selector = source.selector();

    let mut stream = if no_format {
        source.text_stream(ConsoleHistory::Replay).await
    } else {
        source.line_stream(ConsoleHistory::Replay).await
    };
    *enabled_source = Some(selector);
    cf.console.enable(selector).await?;

    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    while let Some(text) = stream.next().await {
        if no_format {
            write!(output, "{}", text)?;
            output.flush()?;
        } else {
            writeln!(output, "{}", format_console_line(&text))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_source_paths_for_humans() {
        assert_eq!(
            render_source_paths(&["deck:bcCam", "deck:other"], false),
            "Path\n----\ndeck:bcCam\ndeck:other\n"
        );
    }

    #[test]
    fn renders_empty_source_catalog_for_humans() {
        assert_eq!(
            render_source_paths(&[], false),
            "No sourced console sources found.\n"
        );
    }

    #[test]
    fn renders_source_paths_as_csv() {
        assert_eq!(
            render_source_paths(&["deck:bcCam", "deck,other"], true),
            "path\ndeck:bcCam\n\"deck,other\"\n"
        );
    }
}
