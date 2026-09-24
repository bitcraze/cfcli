use anyhow::Result;
use crazyflie_lib::Crazyflie;
use std::io::Write;
use crate::utils::display::{csv_row, print_table, table, value_to_csv_string};
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

pub async fn print(cf: &Crazyflie, names: &str, period: u64, csv: bool) -> Result<()> {

  let mut block = cf.log.create_block().await?;

  let name_list: Vec<String> = names.split(",").map(|s| s.to_string()).collect();
  for name in &name_list {
      block.add_variable(name).await?;
  }

  let stream = block
      .start(crazyflie_lib::subsystems::log::LogPeriod::from_millis(
          period,
      )?)
      .await?;

  if csv {
    let mut header: Vec<&str> = vec!["timestamp_ms"];
    for n in &name_list { header.push(n); }
    csv_row(&header);
    let mut stdout = std::io::stdout();
    while let Ok(data) = stream.next().await {
      let mut row: Vec<String> = Vec::with_capacity(name_list.len() + 1);
      row.push(data.timestamp.to_string());
      for n in &name_list {
        let v = data.data.get(n)
          .map(|v| value_to_csv_string(v))
          .unwrap_or_default();
        row.push(v);
      }
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
