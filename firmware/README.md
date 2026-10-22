# ELD firmware

Rust firmware for an ESP32-based Electronic Logging Device (ELD). Reads a
vehicle's J1939 CAN bus (heavy trucks), decodes engine/vehicle telemetry,
verifies mileage, logs to SD, and uploads when connectivity is available.

Target: **LILYGO T-SIM7670G-S3-Standard** (ESP32-S3 Xtensa), `std` on
ESP-IDF v5.5.3 via `esp-idf-svc`.

## Architecture

`main.rs` is intentionally tiny — it builds an `App` and runs it. `App`
(`app.rs`) is the orchestrator: it owns every subsystem and the main loop. No
subsystem talks to another directly; data flows one way:

```
CAN frame → J1939 decoder → Telemetry → Verifier → Trip engine → Storage → Comms
```

Subsystems (each its own module tree):

| Module       | Job                                                        |
|--------------|------------------------------------------------------------|
| `config`     | pins, constants, profiles, feature flags                   |
| `device`     | identity, boot/reset reason, diagnostics, power, health    |
| `can`        | TWAI driver, bitrate scan, raw frames, bus stats           |
| `j1939`      | 29-bit ID parse, PGN/SPN registry, decode, request, discovery |
| `telemetry`  | current truth: rpm/speed/fuel/hours/odometer + age/source  |
| `verifier`   | mileage verification (odometer vs speed-integration vs GPS)|
| `trip`       | trip lifecycle + per-trip mileage/events/summary           |
| `gps`        | UART NMEA, fix quality, distance integration               |
| `storage`    | SD mount, structured log records, rotation, recovery       |
| `comms`      | WiFi / Bluetooth, upload, offline queue, retry             |
| `logging`    | console, structured events, heartbeat                      |

### A note on PGNs

J1939 PGNs are **SAE-standardized** — `0xF004` is EEC1 (RPM) on every compliant
truck regardless of make. What varies per manufacturer is *availability*: which
PGNs an ECU broadcasts, source addresses, broadcast vs on-request, and the
proprietary range (`0xFF00–0xFFFF`). So the firmware keeps a known-PGN registry
and **discovers** what a given bus actually offers (passive scan → request
missing → build a `BusProfile`) rather than assuming a fixed set exists.

## Build order (phases)

1. **CAN + J1939 scanner** — port of the Arduino scanner: driver, bitrate scan,
   ID parse, PGN decode, request-missing, heartbeat logs.
2. **Telemetry state** — latest values + data age + source + confidence.
3. **SD logging** — boot / raw CAN / decoded signal / heartbeat records.
4. **Trip + mileage** — start/end, odometer delta, speed-integration fallback, verifier.
5. **GPS** — NMEA parse, distance integration, mileage cross-check.
6. **Comms** — WiFi upload, offline queue, Bluetooth/phone status.
7. **Hardware diagnostics** — power sense, pinset detection, transceiver strategy.

## Hardware

- LILYGO T-SIM7670G-S3-Standard with onboard SIM7670G, GNSS, and TF socket.
- External CAN transceiver (e.g. SN65HVD230) on **GPIO15 (TX) / GPIO16 (RX)**.
- Onboard TF socket: **SCK 12 / MOSI 11 / MISO 13 / CS 10**.
- SIM7670G AT UART: **TX 4 / RX 5 / PWRKEY 46**; GNSS NMEA is on
  **TX 48 / RX 45** from the ESP32-S3 side.

For bench bring-up without a SIM, `ENABLE_CELLULAR=true` powers the modem and
GNSS while `ENABLE_CELLULAR_UPLINK=false` prevents pointless registration/reset
loops. Set the uplink flag only after inserting the SIM with board power off.
- OBD/J1939 bus at **250 kbit/s or 500 kbit/s** (scanned at startup).
- Wi-Fi/BLE remain on-chip and can be enabled independently after board bring-up.

> Physical OBD pin switching (6/14 vs 3/11) is **not** automatic unless the
> hardware has multiple transceivers or a mux. Software can configure what's
> wired; it can't re-route pins that aren't.

> **Status:** this firmware has never been built in this public repo's CI and
> has **not been validated on hardware**. Its pure logic (J1939, TP, OBD-II,
> OTA policy, AT parsing, mileage) is host-tested in `../firmware-tests`.

## Develop

Prereqs: the `esp` Rust toolchain (`rust-toolchain.toml` pins it) and
`espflash` (`cargo install espflash`). First build downloads ESP-IDF +
toolchain into `.embuild` (several GB, cached).

```bash
cargo build                 # debug
cargo build --release       # release

tools/flash.sh              # release build + flash + monitor
tools/flash.sh --debug      # debug build + flash + monitor
tools/flash.sh -p /dev/ttyUSB0

tools/monitor.sh            # serial monitor only
tools/clean-env.sh          # cargo clean
tools/clean-env.sh --full   # also wipe .embuild (forces toolchain re-download)
```

For a provisioned device, copy `.env.example` to `.env.local` (gitignored) and
set its unique ingest credential. `tools/flash.sh` loads these values at build time.
Leave `ELD_CELLULAR_UPLINK` unset until the SIM and public HTTPS route are
ready; then set `ELD_CELLULAR_UPLINK=1`, rebuild, and flash.

Decode captured logs:

```bash
tools/monitor.sh | tee serial.log
tools/decode-log.py serial.log
tools/decode-log.py --json serial.log   # JSONL
```

## Layout

```
src/
  main.rs        entry point — build App, boot, run
  app.rs         orchestrator: owns subsystems + main loop
  error.rs       EldError + Result
  time.rs        monotonic millis since boot
  config/ device/ can/ j1939/ telemetry/ verifier/
  trip/ gps/ storage/ comms/ logging/
tools/           flash / monitor / clean / log-decode / OTA key + manifest signing
  partitions.csv   dual-OTA layout on the board's 16MB flash
sdkconfig.defaults
```
