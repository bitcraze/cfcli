use anyhow::{bail, Result};
use crazyflie_lib::Crazyflie;
use crazyflie_lib::Value;
use crazyflie_lib::ValueType;
use std::collections::HashMap;

pub async fn list(cf: &Crazyflie) -> Result<()> {
  
    println!("{: <30} | {: <6} | {: <6}", "Name", "Access", "Value");
    println!("{0:-<30}-|-{0:-<6}-|-{0:-<6}", "");

    for name in cf.param.names() {
        let value: crazyflie_lib::Value = cf.param.get(&name).await?;
        let writable = if cf.param.is_writable(&name)? {
            "RW"
        } else {
            "RO"
        };

        println!("{: <30} | {: ^6} | {:?}", name, writable, value);
    }

    Ok(())
}

pub async fn get(cf: &Crazyflie, names: &str) -> Result<()> {

  println!("{: <30} | {: <6} | {: <6}", "Name", "Access", "Value");
  println!("{0:-<30}-|-{0:-<6}-|-{0:-<6}", "");

  let name_list: Vec<&str> = names.split(',').collect();
  for name in name_list {
    let value: Value = cf.param.get(name).await?;
    let writable = if cf.param.is_writable(&name)? {
      "RW"
    } else {
      "RO"
    };
    println!("{: <30} | {: ^6} | {:?}", name, writable, value);
  }

  Ok(())
}

pub async fn set(cf: &Crazyflie, param_list: &HashMap<String, String>) -> Result<()> {

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
}


  Ok(())
}

