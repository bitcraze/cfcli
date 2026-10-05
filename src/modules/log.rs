use anyhow::{anyhow, Result};
use crazyflie_lib::subsystems::log::{LogData, LogPeriod, LogStream};
use crazyflie_lib::Crazyflie;
use std::io::Write;
use crate::utils::display::{csv_row, print_table, table, table_from_records, value_to_csv_string};
use tabled::Tabled;

/// One row of `log list`.
#[derive(Tabled)]
struct LogVariable {
  #[tabled(rename = "Name")]
  name: String,
  #[tabled(rename = "Type")]
  var_type: String,
}

pub async fn list(cf: &Crazyflie, csv: bool) -> Result<()> {
  if csv {
    println!("name,type");
    for name in cf.log.names() {
      let var_type = cf.log.get_type(&name)?;
      csv_row(&[&name, &format!("{:?}", var_type)]);
    }
  } else {
    let mut rows = Vec::new();
    for name in cf.log.names() {
      let var_type = cf.log.get_type(&name)?;
      rows.push(LogVariable { name, var_type: format!("{:?}", var_type) });
    }
    print_table(&table(&rows));
  }

  Ok(())
}

/// Log the variables (comma-separated) every `period` ms.
pub async fn start(cf: &Crazyflie, names: &[String], period: u64) -> Result<LogStream> {
  let mut block = cf.log.create_block().await?;
  for name in names {
      block.add_variable(name).await?;
  }
  Ok(block.start(LogPeriod::from_millis(period)?).await?)
}

/// One sample of the variables.
pub async fn sample(cf: &Crazyflie, names: &[String], period: u64) -> Result<LogData> {
  let stream = start(cf, names, period).await?;
  let data = stream.next().await?;
  // The block is gone with the connection anyway.
  let _ = stream.stop().await;
  Ok(data)
}

/// The values of a sample as plain numbers, in the order of `names`.
pub fn values(data: &LogData, names: &[String]) -> Vec<String> {
  names
    .iter()
    .map(|name| data.data.get(name).map(value_to_csv_string).unwrap_or_default())
    .collect()
}

/// Split a comma-separated list of variable names.
pub fn split_names(names: &str) -> Vec<String> {
  names.split(',').map(|s| s.to_string()).collect()
}

/// Let the user pick variables to log, returned comma-separated.
pub fn pick_names(cf: &Crazyflie) -> Result<String> {
  let selected = inquire::MultiSelect::new("Select variables to log:", cf.log.names())
    .prompt()
    .map_err(|_| anyhow!("No variables selected"))?;
  Ok(selected.join(","))
}

/// Print one sample: a table with a column per variable, or a CSV row with
/// the timestamp in front like `log print --csv`.
pub async fn print_once(cf: &Crazyflie, names: &str, period: u64, csv: bool) -> Result<()> {
  let name_list = split_names(names);
  let data = sample(cf, &name_list, period).await?;
  let values = values(&data, &name_list);
  if csv {
    let mut header: Vec<&str> = vec!["timestamp_ms"];
    header.extend(name_list.iter().map(String::as_str));
    csv_row(&header);
    let timestamp = data.timestamp.to_string();
    let mut row: Vec<&str> = vec![&timestamp];
    row.extend(values.iter().map(String::as_str));
    csv_row(&row);
  } else {
    print_table(&table_from_records(&name_list, &[values]));
  }
  Ok(())
}

pub async fn print(cf: &Crazyflie, names: &str, period: u64, csv: bool) -> Result<()> {
  let name_list = split_names(names);
  let stream = start(cf, &name_list, period).await?;

  if csv {
    let mut header: Vec<&str> = vec!["timestamp_ms"];
    for n in &name_list { header.push(n); }
    csv_row(&header);
    let mut stdout = std::io::stdout();
    while let Ok(data) = stream.next().await {
      let mut row = vec![data.timestamp.to_string()];
      row.extend(values(&data, &name_list));
      let row_refs: Vec<&str> = row.iter().map(|s| s.as_str()).collect();
      csv_row(&row_refs);
      // Flush per row so consumers piping `log print --csv` see samples in
      // real time instead of in stdio-buffered chunks.
      let _ = stdout.flush();
    }
  } else {
    while let Ok(data) = stream.next().await {
        println!("{:?}", data);
    }
  }

  Ok(())
}
