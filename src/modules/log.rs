use anyhow::{anyhow, Result};
use crazyflie_connection_server::{Crazyflie, LogStream};
use crazyflie_lib::subsystems::log::LogData;
use crazyflie_lib::{Value, ValueType};
use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;
use crate::utils::connection::log_type;
use crate::utils::display::{csv_row, print_table, table, table_from_records, value_to_csv_string, Column, StreamTable};
use tabled::settings::{object::Columns, Alignment, Modify};
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
      let var_type = log_type(cf, &name)?;
      csv_row(&[&name, &format!("{:?}", var_type)]);
    }
  } else {
    let mut rows = Vec::new();
    for name in cf.log.names() {
      let var_type = log_type(cf, &name)?;
      rows.push(LogVariable { name, var_type: format!("{:?}", var_type) });
    }
    print_table(&table(&rows));
  }

  Ok(())
}

/// Samples of a log subscription, with the values typed like the variables.
/// Dropping it removes the log block.
pub struct Samples {
  stream: LogStream,
  types: HashMap<String, ValueType>,
}

impl Samples {
  pub async fn next(&mut self) -> Result<LogData> {
    let sample = self.stream.next().await?;
    let data = sample
      .data
      .into_iter()
      .map(|(name, value)| {
        let value_type = self.types.get(&name).copied().unwrap_or(ValueType::F64);
        (name, Value::from_f64_lossy(value_type, value))
      })
      .collect();
    Ok(LogData { timestamp: sample.timestamp, data })
  }
}

/// Log the variables every `period` ms. The server creates the log block.
pub async fn start(cf: &Crazyflie, names: &[String], period: u64) -> Result<Samples> {
  let types = names.iter().map(|name| Ok((name.clone(), log_type(cf, name)?))).collect::<Result<_>>()?;
  let names: Vec<&str> = names.iter().map(String::as_str).collect();
  let stream = cf.log.subscribe(&names, Duration::from_millis(period)).await?;
  Ok(Samples { stream, types })
}

/// One sample of the variables.
pub async fn sample(cf: &Crazyflie, names: &[String], period: u64) -> Result<LogData> {
  start(cf, names, period).await?.next().await
}

/// The values of a sample as plain numbers at full precision, in the order
/// of `names`. For CSV.
pub fn values(data: &LogData, names: &[String]) -> Vec<String> {
  names
    .iter()
    .map(|name| data.data.get(name).map(value_to_csv_string).unwrap_or_default())
    .collect()
}

/// Decimals of a float in a table: millimetres, millivolts, milli-g and
/// thousandths of a degree, at or below what the sensors resolve. `--csv`
/// keeps the full value.
const DECIMALS: usize = 3;
/// Width of the time column: a u32 of milliseconds.
const TIME_WIDTH: usize = 10;
/// Width a float column starts with: -999.999, enough for most values. A
/// larger one (thrust, say) widens its column once.
const FLOAT_WIDTH: usize = 8;

/// A value as shown in a table: floats with [`DECIMALS`] decimals, so that
/// a column keeps its width and its decimal points line up.
fn display_value(value: &Value) -> String {
  match value {
    Value::F16(x) => format!("{:.*}", DECIMALS, x.to_f32()),
    Value::F32(x) => format!("{:.*}", DECIMALS, x),
    Value::F64(x) => format!("{:.*}", DECIMALS, x),
    other => value_to_csv_string(other),
  }
}

/// The values of a sample as shown in a table, in the order of `names`.
pub fn display_values(data: &LogData, names: &[String]) -> Vec<String> {
  names
    .iter()
    .map(|name| data.data.get(name).map(display_value).unwrap_or_default())
    .collect()
}

/// The width any value of the type fits in.
fn type_width(value_type: ValueType) -> usize {
  match value_type {
    ValueType::U8 => 3,
    ValueType::I8 => 4,
    ValueType::U16 => 5,
    ValueType::I16 => 6,
    ValueType::U32 => 10,
    ValueType::I32 => 11,
    ValueType::U64 | ValueType::I64 => 20,
    ValueType::F16 | ValueType::F32 | ValueType::F64 => FLOAT_WIDTH,
  }
}

/// The columns of streamed samples: the Crazyflie's time, then a column per
/// variable in the order asked for, as wide as its type needs.
pub fn stream_columns<'a>(cf: &Crazyflie, names: &'a [String]) -> Vec<Column<'a>> {
  let mut columns = vec![Column { name: "Time (ms)", width: TIME_WIDTH, right: true }];
  columns.extend(names.iter().map(|name| Column {
    name,
    width: log_type(cf, name).map_or(FLOAT_WIDTH, type_width),
    right: true,
  }));
  columns
}

/// A streamed sample as table fields: the time, then the values.
pub fn stream_fields(data: &LogData, names: &[String]) -> Vec<String> {
  let mut fields = vec![data.timestamp.to_string()];
  fields.extend(display_values(data, names));
  fields
}

/// A sample as CSV fields: the time, then the values at full precision.
pub fn csv_fields(data: &LogData, names: &[String]) -> Vec<String> {
  let mut fields = vec![data.timestamp.to_string()];
  fields.extend(values(data, names));
  fields
}

/// Print samples as a table, with the values right-aligned from column
/// `first_value` on.
pub fn print_sample_table(header: &[String], rows: &[Vec<String>], first_value: usize) {
  let mut table = table_from_records(header, rows);
  table.with(Modify::new(Columns::new(first_value..)).with(Alignment::right()));
  print_table(&table);
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
  if csv {
    let mut header: Vec<&str> = vec!["timestamp_ms"];
    header.extend(name_list.iter().map(String::as_str));
    csv_row(&header);
    let row = csv_fields(&data, &name_list);
    csv_row(&row.iter().map(String::as_str).collect::<Vec<_>>());
  } else {
    print_sample_table(&name_list, &[display_values(&data, &name_list)], 0);
  }
  Ok(())
}

pub async fn print(cf: &Crazyflie, names: &str, period: u64, csv: bool) -> Result<()> {
  let name_list = split_names(names);
  let mut stream = start(cf, &name_list, period).await?;

  if csv {
    let mut header: Vec<&str> = vec!["timestamp_ms"];
    for n in &name_list { header.push(n); }
    csv_row(&header);
    let mut stdout = std::io::stdout();
    while let Ok(data) = stream.next().await {
      let row = csv_fields(&data, &name_list);
      let row_refs: Vec<&str> = row.iter().map(|s| s.as_str()).collect();
      csv_row(&row_refs);
      // Flush per row so consumers piping `log print --csv` see samples in
      // real time instead of in stdio-buffered chunks.
      let _ = stdout.flush();
    }
  } else {
    let mut table = StreamTable::new(&stream_columns(cf, &name_list));
    while let Ok(data) = stream.next().await {
      let row = stream_fields(&data, &name_list);
      table.row(&row.iter().map(String::as_str).collect::<Vec<_>>());
    }
  }

  Ok(())
}
