use anyhow::{bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::Value;
use crazyflie_lib::ValueType;
use std::collections::HashMap;
use std::collections::HashSet;
use crate::error::CliError;
use crate::utils::display::{csv_row, value_to_csv_string};

/// CSV header shared by `param list` and `param get` so consumers see the
/// same columns from both commands. Ordering matters — the `print_csv_row`
/// helper below emits fields in this order.
const PARAM_CSV_HEADER: &str = "name,access,persistent,default,stored_value,value";

/// Build and emit one CSV row describing a single parameter. Reads:
/// access, persistence state, stored/default values (if persistent), and
/// the current value. Empty cells for fields that don't apply (e.g.
/// non-persistent params have empty default/stored_value).
async fn print_csv_row(cf: &Crazyflie, name: &str) -> Result<()> {
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

    csv_row(&[name, writable, persistent, &default_s, &stored_s, &value_to_csv_string(&value)]);
    Ok(())
}

/// Verify each `name` is in the parameter TOC. Bails with `CliError::NotFound`
/// (exit 20) on the first miss. Avoids relying on string-matching the lib's
/// `ParamError` messages downstream in `classify_exit_code`.
fn check_params_exist<'a, I: IntoIterator<Item = &'a str>>(cf: &Crazyflie, names: I) -> Result<()> {
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

    println!("{: <30} | {: <6} | {: <10} | {: <12}", "Name", "Access", "Persistent", "Value/Stored");
    println!("{0:-<30}-|-{0:-<6}-|-{0:-<10}-|-{0:-<12}", "");

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
        println!("{: <30} | {: ^6} | {: <10} | {}", name, writable, persistent, value_str);
    }

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

    println!("{: <30} | {: <6} | {: <10} | {: <15} | {: <15} | {: <6}", "Name", "Access", "Persistent", "Default", "Stored Value", "Value");
    println!("{0:-<30}-|-{0:-<6}-|-{0:-<10}-|-{0:-<15}-|-{0:-<15}-|-{0:-<6}", "");

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

        println!("{: <30} | {: ^6} | {: <10} | {: <15} | {: <15} | {:?}", name, writable, persistent, default_str, stored_str, value);
    }

    Ok(())
}

pub async fn set(cf: &Crazyflie, param_list: &HashMap<String, String>, store: bool) -> Result<()> {
  check_params_exist(cf, param_list.keys().map(|s| s.as_str()))?;

  for (name, value) in param_list {
    match cf.param.get_type(&name) {
      Ok(ValueType::U8) => {
        let value:u8 = value.parse()?;
        cf.param.set(name, value).await?;
    },
    Ok(ValueType::U16) => {
      let value:u16 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::U32) => {
      let value:u32 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::U64) => {
      let value:u64 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::I8) => {
      let value:i8 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::I16) => {
      let value:i16 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::I32) => {
      let value:i32 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::I64) => {
      let value:i64 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::F16) => {
      let value:f32 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::F32) => {
      let value:f32 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Ok(ValueType::F64) => {
      let value:f64 = value.parse()?;
      cf.param.set(name, value).await?;
    },
    Err(e) => bail!("Failed to get type for parameter '{}': {}", name, e),
  }

    if store {
      cf.param.persistent_store(name).await?;
      println!("Stored {} to EEPROM", name);
    }
  }

  Ok(())
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
