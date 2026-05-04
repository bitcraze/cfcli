# Logging

The logging sub-system provides a way to peri.odically sample variables from the Crazyflie.

For more informaiton on how to use the logging and parameter sub-systems see [this link](https://www.bitcraze.io/documentation/repository/crazyflie-firmware/master/userguides/logparam/)

## List available variables

```bash
cfcli log list
```

This will produce an output similar to this:

```text
Name                           | Type 
-------------------------------|------
DTR_P2P.rx_state               | U8
DTR_P2P.tx_state               | U8
acc.x                          | F32
acc.y                          | F32
acc.z                          | F32
activeMarker.btSns             | U8
activeMarker.i2cOk             | U8
baro.asl                       | F32
...
```

Where the `Name` column is the variable name (group.name) and the `Type` column
describes the data type of the variable.

## Log variables

There's two ways to log variables, either specify exactly which variables to log on the
command line, or leave it blank to get an interactive prompt with all the available variables
where you can select which ones to log.

To print the values of the `acc.x` and `acc.y` variables at each 10 ms use the following command:

```bash
cfcli log print acc.x,acc.y -p 10
```

The period defaults to 100 ms if `-p`/`--period` is omitted.

This will produce an output similar to this:

```text
LogData { timestamp: 377433600, data: {"acc.x": F32(0.02040638), "acc.y": F32(-0.011233097)} }
LogData { timestamp: 377436160, data: {"acc.x": F32(0.018765358), "acc.y": F32(-0.01344875)} }
```

We need to be able to set period without variables...

## Set up base station (i.e set channel and download calib data?)
