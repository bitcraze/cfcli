use anyhow::{bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::Value;
use crazyflie_lib::ValueType;
use std::collections::HashMap;

pub async fn list(cf: &Crazyflie) -> Result<()> {

    println!("{: <30} | {: <6} | {: <10} | {: <12}", "Name", "Access", "Persistent", "Value/Stored");
    println!("{0:-<30}-|-{0:-<6}-|-{0:-<10}-|-{0:-<12}", "");

    for name in cf.param.names() {
        let value: crazyflie_lib::Value = cf.param.get(&name).await?;
        let writable = if cf.param.is_writable(&name)? {
            "RW"
        } else {
            "RO"
        };

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

pub async fn get(cf: &Crazyflie, names: &str) -> Result<()> {

  println!("{: <30} | {: <6} | {: <10} | {: <15} | {: <15} | {: <6}", "Name", "Access", "Persistent", "Default", "Stored Value", "Value");
  println!("{0:-<30}-|-{0:-<6}-|-{0:-<10}-|-{0:-<15}-|-{0:-<15}-|-{0:-<6}", "");

  let name_list: Vec<&str> = names.split(',').collect();
  for name in name_list {
    let value: Value = cf.param.get(name).await?;
    let writable = if cf.param.is_writable(&name)? {
      "RW"
    } else {
      "RO"
    };

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
  let name_list: Vec<&str> = names.split(',').collect();
  for name in name_list {
    cf.param.persistent_store(name).await?;
    println!("Stored {} to EEPROM", name);
  }
  Ok(())
}

pub async fn clear(cf: &Crazyflie, names: &str) -> Result<()> {
  let name_list: Vec<&str> = names.split(',').collect();
  for name in name_list {
    cf.param.persistent_clear(name).await?;
    println!("Cleared {} from EEPROM", name);
  }
  Ok(())
}
