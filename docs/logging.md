# Logging

The logging sub-system provides a way to peri.odically sample variables from the Crazyflie.

For more informaiton on how to use the logging and parameter sub-systems see [this link](https://www.bitcraze.io/documentation/repository/crazyflie-firmware/master/userguides/logparam/)

## List available variables

```bash
cfcli log list
```

This will produce an output similar to this:

```text
Name             | Type
-----------------+------
DTR_P2P.rx_state | U8
DTR_P2P.tx_state | U8
acc.x            | F32
acc.y            | F32
acc.z            | F32
activeMarker.btSns             | U8
activeMarker.i2cOk             | U8
baro.asl                       | F32
...
```

Where the `Name` column is the variable name (group.name) and the `Type` column
describes the data type of the variable.

For machine-readable output, add the global `--csv` flag:

```bash
cfcli --csv log list
```

The first line is a header row, then one variable per line:

```text
name,type
DTR_P2P.rx_state,U8
DTR_P2P.tx_state,U8
acc.x,F32
```

## Log variables

There's two ways to log variables, either specify exactly which variables to log on the
command line, or leave it blank to get an interactive prompt with all the available variables
where you can select which ones to log.

To print the values of the `acc.x` and `acc.y` variables at each 10 ms use the following command:

```bash
cfcli log print acc.x,acc.y -p 10
```

The period defaults to 100 ms if `-p`/`--period` is omitted.

This prints a table with the Crazyflie's time and a column per variable, one
row per sample:

```text
Time (ms)  | acc.x        | acc.y
-----------+--------------+--------------
377433600  | 0.02040638   | -0.011233097
377436160  | 0.018765358  | -0.01344875
```

The columns can't be sized from samples that haven't arrived yet, so a value
longer than its column pushes the rest of its row to the right.

### CSV output

For piping into another tool or capturing to a file, use the global `--csv`
flag. The first line is a header row (`timestamp_ms` plus one column per
requested variable), and each sample is one row with plain numeric values:

```bash
cfcli --csv log print acc.x,acc.y -p 10
```

```text
timestamp_ms,acc.x,acc.y
377433600,0.02040638,-0.011233097
377436160,0.018765358,-0.01344875
```

Rows are flushed per sample, so consumers see data in real time rather than in
stdio-buffered chunks.

### Stop streaming after a fixed duration

`log print` is a streaming command — by default it runs until the link is
broken. Combine it with the global `--timeout` flag to stop cleanly after a
fixed wall-clock duration:

```bash
cfcli --timeout 3000 log print acc.x,acc.y -p 10
```

When `--timeout` fires on a streaming command, the process exits **0** (the
timer is the intended way to stop it). This is the recommended pattern when
running `cfcli log print` from a script or CI step.

### One sample

Add `--once` to read one sample and stop, shown as a table with a column per
variable:

```bash
cfcli log print pm.vbat,pm.state --once
```

```text
pm.vbat  | pm.state
---------+----------
4.196481 | 2
```

With `--csv` it is the same header and row as `log print --csv` gives for the
first sample. `--once` is not a streaming command, so a `--timeout` that fires
before the sample arrives exits **40**.

## Set up base station (i.e set channel and download calib data?)
