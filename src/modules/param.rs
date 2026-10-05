use anyhow::{anyhow, bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::Value;
use crazyflie_lib::ValueType;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::error::CliError;
use crate::utils::display::{csv_row, print_table, table, value_to_csv_string};
use tabled::settings::{object::Columns, Alignment, Modify};
use tabled::Tabled;

/// One row of `param list`.
#[derive(Tabled)]
struct ParamListRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Access")]
    access: String,
    #[tabled(rename = "Persistent")]
    persistent: String,
    #[tabled(rename = "Value/Stored")]
    value: String,
}

/// One row of `param get`, which also shows the persisted default and value.
#[derive(Tabled)]
pub struct ParamGetRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Access")]
    access: String,
    #[tabled(rename = "Persistent")]
    persistent: String,
    #[tabled(rename = "Default")]
    default: String,
    #[tabled(rename = "Stored Value")]
    stored: String,
    #[tabled(rename = "Value")]
    value: String,
}

/// CSV header shared by `param list` and `param get` so consumers see the
/// same columns from both commands. Ordering matters — the `csv_fields`
/// helper below returns fields in this order.
pub const PARAM_CSV_HEADER: &str = "name,access,persistent,default,stored_value,value";

/// Emit one CSV row describing a single parameter.
async fn print_csv_row(cf: &Crazyflie, name: &str) -> Result<()> {
    let fields = csv_fields(cf, name).await?;
    let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
    csv_row(&fields);
    Ok(())
}

/// The CSV fields describing a single parameter. Reads: access,
/// persistence state, stored/default values (if persistent), and the
/// current value. Empty cells for fields that don't apply (e.g.
/// non-persistent params have empty default/stored_value).
pub async fn csv_fields(cf: &Crazyflie, name: &str) -> Result<Vec<String>> {
    let value: Value = cf.param.get(name).await?;
    let writable = if cf.param.is_writable(name)? { "RW" } else { "RO" };

    let (persistent, default_s, stored_s) = if cf.param.is_persistent(name).await? {
        match cf.param.persistent_get_state(name).await {
            Ok(state) => {
                let default = value_to_csv_string(&state.default_value);
                let stored = match state.stored_value {
                    Some(v) => value_to_csv_string(&v),
                    None => String::new(),
                };
                ("yes", default, stored)
            }
            Err(_) => ("error", String::new(), String::new()),
        }
    } else {
        ("no", String::new(), String::new())
    };

    Ok(vec![
        name.to_string(),
        writable.to_string(),
        persistent.to_string(),
        default_s,
        stored_s,
        value_to_csv_string(&value),
    ])
}

/// Verify each `name` is in the parameter TOC. Bails with `CliError::NotFound`
/// (exit 20) on the first miss. Avoids relying on string-matching the lib's
/// `ParamError` messages downstream in `classify_exit_code`.
pub fn check_params_exist<'a, I: IntoIterator<Item = &'a str>>(cf: &Crazyflie, names: I) -> Result<()> {
    let toc: HashSet<String> = cf.param.names().into_iter().collect();
    for name in names {
        if !toc.contains(name) {
            bail!(CliError::NotFound(format!("parameter '{}'", name)));
        }
    }
    Ok(())
}

pub async fn list(cf: &Crazyflie, csv: bool) -> Result<()> {
    if csv {
        println!("{}", PARAM_CSV_HEADER);
        for name in cf.param.names() {
            print_csv_row(cf, &name).await?;
        }
        return Ok(());
    }

    let mut rows = Vec::new();
    for name in cf.param.names() {
        let value: Value = cf.param.get(&name).await?;
        let writable = if cf.param.is_writable(&name)? { "RW" } else { "RO" };

        let (persistent, value_str) = if cf.param.is_persistent(&name).await? {
            match cf.param.persistent_get_state(&name).await {
                Ok(state) if state.is_stored => {
                    let stored_val = state.stored_value.unwrap();
                    ("Stored", format!("{:?}/{:?}", value, stored_val))
                }
                Ok(_) => ("Yes", format!("{:?}", value)),
                Err(_) => ("Error", format!("{:?}", value)),
            }
        } else {
            ("", format!("{:?}", value))
        };
        rows.push(ParamListRow {
            name,
            access: writable.to_string(),
            persistent: persistent.to_string(),
            value: value_str,
        });
    }

    // `Access` holds a short RW/RO flag; centring it keeps the column from
    // looking ragged against the much wider name column.
    let mut table = table(&rows);
    table.with(Modify::new(Columns::one(1)).with(Alignment::center()));
    print_table(&table);

    Ok(())
}

pub async fn get(cf: &Crazyflie, names: &str, csv: bool) -> Result<()> {
    check_params_exist(cf, names.split(','))?;

    if csv {
        println!("{}", PARAM_CSV_HEADER);
        for name in names.split(',') {
            print_csv_row(cf, name).await?;
        }
        return Ok(());
    }

    print_get_table(&get_rows(cf, names).await?, 1);
    Ok(())
}

/// Print `param get` rows. `access_column` is the index of the Access
/// column, which is centred.
pub fn print_get_table<R: Tabled>(rows: &[R], access_column: usize) {
    let mut table = table(rows);
    table.with(Modify::new(Columns::one(access_column)).with(Alignment::center()));
    print_table(&table);
}

/// The `param get` rows for `names` (comma-separated, checked to exist).
pub async fn get_rows(cf: &Crazyflie, names: &str) -> Result<Vec<ParamGetRow>> {
    let mut rows = Vec::new();
    for name in names.split(',') {
        let value: Value = cf.param.get(name).await?;
        let writable = if cf.param.is_writable(&name)? { "RW" } else { "RO" };

        let (persistent, default_str, stored_str) = if cf.param.is_persistent(name).await? {
            match cf.param.persistent_get_state(name).await {
                Ok(state) => {
                    let stored = if state.is_stored { "Yes".to_string() } else { "No".to_string() };
                    let default = format!("{:?}", state.default_value);
                    let stored_val = match state.stored_value {
                        Some(v) => format!("{:?}", v),
                        None => String::new(),
                    };
                    (stored, default, stored_val)
                }
                Err(_) => ("Error".to_string(), String::new(), String::new()),
            }
        } else {
            (String::new(), String::new(), String::new())
        };

        rows.push(ParamGetRow {
            name: name.to_string(),
            access: writable.to_string(),
            persistent,
            default: default_str,
            stored: stored_str,
            value: format!("{:?}", value),
        });
    }
    Ok(rows)
}

pub async fn set(cf: &Crazyflie, param_list: &HashMap<String, String>, store: bool) -> Result<()> {
  check_params_exist(cf, param_list.keys().map(|s| s.as_str()))?;

  for (name, value) in param_list {
    set_value(cf, name, value).await?;

    if store {
      cf.param.persistent_store(name).await?;
      println!("Stored {} to EEPROM", name);
    }
  }

  Ok(())
}

/// Set one parameter from its text form, parsed as the parameter's type.
pub async fn set_value(cf: &Crazyflie, name: &str, value: &str) -> Result<()> {
    match cf.param.get_type(name) {
        Ok(ValueType::U8) => {
            let value: u8 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::U16) => {
            let value: u16 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::U32) => {
            let value: u32 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::U64) => {
            let value: u64 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::I8) => {
            let value: i8 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::I16) => {
            let value: i16 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::I32) => {
            let value: i32 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::I64) => {
            let value: i64 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::F16) => {
            let value: f32 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::F32) => {
            let value: f32 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Ok(ValueType::F64) => {
            let value: f64 = value.parse()?;
            cf.param.set(name, value).await?;
        }
        Err(e) => bail!("Failed to get type for parameter '{}': {}", name, e),
    }
    Ok(())
}

/// Let the user pick parameters, returned comma-separated.
pub async fn pick_names(cf: &Crazyflie, message: &str) -> Result<String> {
    let selected = inquire::MultiSelect::new(message, cf.param.names())
        .prompt()
        .map_err(|_| anyhow!("No parameters selected"))?;
    Ok(selected.join(","))
}

/// Let the user pick persistent parameters, returned comma-separated.
pub async fn pick_persistent(cf: &Crazyflie, message: &str) -> Result<String> {
    let mut persistent = Vec::new();
    for name in cf.param.names() {
        if cf.param.is_persistent(&name).await? {
            persistent.push(name);
        }
    }
    let selected = inquire::MultiSelect::new(message, persistent)
        .prompt()
        .map_err(|_| anyhow!("No parameters selected"))?;
    Ok(selected.join(","))
}

/// Let the user pick writable parameters and type a value for each.
pub async fn pick_values(cf: &Crazyflie) -> Result<HashMap<String, String>> {
    let writable: Vec<String> = cf
        .param
        .names()
        .into_iter()
        .filter(|name| cf.param.is_writable(name).unwrap_or(false))
        .collect();
    let selected = inquire::MultiSelect::new("Select parameters to set:", writable)
        .prompt()
        .map_err(|_| anyhow!("No parameters selected"))?;

    let mut values = HashMap::new();
    for name in selected {
        let current: Value = cf.param.get(&name).await?;
        let value = inquire::Text::new(&format!("[{}] {:?}:", name, current))
            .prompt()
            .map_err(|_| anyhow!("No value entered for parameter '{}'", name))?;
        values.insert(name, value);
    }
    Ok(values)
}

pub async fn store(cf: &Crazyflie, names: &str) -> Result<()> {
  check_params_exist(cf, names.split(','))?;
  for name in names.split(',') {
    cf.param.persistent_store(name).await?;
    println!("Stored {} to EEPROM", name);
  }
  Ok(())
}

pub async fn clear(cf: &Crazyflie, names: &str) -> Result<()> {
  check_params_exist(cf, names.split(','))?;
  for name in names.split(',') {
    cf.param.persistent_clear(name).await?;
    println!("Cleared {} from EEPROM", name);
  }
  Ok(())
}
