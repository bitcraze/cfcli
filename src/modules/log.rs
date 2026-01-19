use anyhow::Result;
use crazyflie_lib::Crazyflie;

pub async fn list(cf: &Crazyflie) -> Result<()> {
  
  println!("{0: <30} | {1: <5}", "Name", "Type");
  println!("{:-<30}-|-{:-<5}", "", "");

  for name in cf.log.names() {
      let var_type = cf.log.get_type(&name)?;

      println!("{0: <30} | {1: <5?}", name, var_type);
  }

    Ok(())
}

pub async fn print(cf: &Crazyflie, names: &str, period: u64) -> Result<()> {

  let mut block = cf.log.create_block().await?;

  for name in names.split(",") {
      block.add_variable(name).await?;
  }

  let stream = block
      .start(crazyflie_lib::subsystems::log::LogPeriod::from_millis(
          period,
      )?)
      .await?;

  while let Ok(data) = stream.next().await {
      println!("{:?}", data);
  }

  Ok(())
}
