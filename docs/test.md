# Test

Test commands exercise the Crazyflie over the radio link to flush out
regressions: connection stability, reboot/selftest behaviour, and the
performance of the radio link itself.

## Stability

Repeatedly run a small set of operations (connect, param read/write, logging)
and report progress. Each iteration picks a test at random until every test has
run the requested number of times.

```text
cfcli test stability [iterations]
```

`iterations` defaults to `10`. The command exits non-zero on the first failure.

## Reboot

Reboot the Crazyflie, reconnect, read `system.selftestPassed`, and capture
console output. Repeat for the requested number of iterations. Results are
written to `reboot_test_results_<timestamp>.json` after every iteration so a
crash mid-run still leaves usable data.

```text
cfcli test reboot [iterations]
```

`iterations` defaults to `10`. The progress bar shows the current phase
(rebooting / connecting / reading selftest / collecting console) and the
running fail count. Console output is collected for ~10 s between reboots so
boot messages, asserts, etc. end up in the JSON file.

## Link performance

Benchmark the CRTP link service (port 15) using its echo, source, and sink
channels:

* **ping** — round-trip latency on the echo channel (1-byte payload)
* **uplink** — fill the sink channel with full-payload packets and time how
  long until the trailing echo round-trips. Measures uplink throughput.
* **downlink** — request `n` source packets and time the responses. Measures
  downlink throughput.
* **echo** — full-payload packets on the echo channel both directions; measures
  achievable throughput when up- and downlink carry data simultaneously.

```text
cfcli test link-perf
```

Run a specific test with `-t/--test`:

```text
cfcli test link-perf -t ping --pings 100
cfcli test link-perf -t uplink --packets 5000
```

Options:

* `-t, --test <all|ping|uplink|downlink|echo>` — which test(s) to run (default: `all`)
* `--packets <N>` — packets per bandwidth test (default: `1000`)
* `--pings <N>` — number of ping samples (default: `10`)

When the link is a radio (not USB) the command also prints the radio link
statistics snapshot taken at the end of the run: link quality, RSSI, uplink /
downlink / radio-send rate, average retries, and power-detector rate.

### CSV output

The global `--csv` flag emits a `metric,value,unit` table instead of the
human-formatted output, so results can be parsed by scripts or pasted into
spreadsheets:

```text
cfcli --csv test link-perf
```

Example rows:

```csv
metric,value,unit
ping_samples,10,count
ping_min_ms,1.234,ms
ping_avg_ms,1.456,ms
ping_max_ms,2.105,ms
uplink_kbit_per_sec,22.400,kbit/s
uplink_bytes_per_sec,2800.000,B/s
uplink_packets_per_sec,93.333,pkt/s
...
link_quality,0.9950,ratio
rssi_dbm,-55.00,dBm
```
