# ELD BLE Sync — GATT service and line protocol

This is the contract between the firmware's BLE relay
(`firmware/src/comms/bluetooth.rs`, `comms/phone.rs`, `comms/mod.rs`) and the
phone transport (`phone-relay/src/bleTransport.ts`). The same line protocol
also runs over the USB serial console (`firmware/src/comms/serial.rs`).

The ELD's SD queue is the source of truth; the phone is a disposable transport.
A record leaves the SD queue only when the phone ACKs a seq range that the
server has already accepted (or reported as a duplicate). A phone that dies,
an app that gets killed, an upload that fails, or a BLE link that drops can
delay a record but never lose it.

```
Truck CAN (J1939 / OBD-II)
  ↓
ELD firmware (ESP32-S3)
  ↓
SD event queue  ← source of truth (per-device monotonic seq, durable)
  ↓
BLE GATT relay (ELD-DONGLE)
  ↓
Phone relay (react-native-ble-plx)
  ↓
Server ingest (POST /ingest, see ingest-contract.md)
  ↓
accepted + duplicate seqs → phone sends ELD ACK ranges → device drops them
```

## GATT service

| Role                  | UUID                                   | Properties | Notes |
| --------------------- | -------------------------------------- | ---------- | ----- |
| Service               | `fc59c4f2-bb03-4328-8361-267acb7e0842` | primary    | |
| Command               | `fc59c4f3-bb03-4328-8361-267acb7e0842` | Write (with response) | max 96 bytes |
| Response              | `fc59c4f4-bb03-4328-8361-267acb7e0842` | Indicate   | CCCD `0x2902` |

- **Advertising.** The primary advertisement carries the flags (`0x06`: LE
  General Discoverable, BR/EDR not supported) and the 128-bit service UUID, so
  iOS and Android can scan with a service filter, including iOS background
  scans. The local name `ELD-DONGLE` is in the scan response, because the UUID,
  flags and name together exceed the 31-byte advertising limit. The phone
  identifies the device by service UUID, not by name.
- **One phone at a time.** The firmware accepts a single connection.
  Advertising stops when a phone connects and restarts about 500 ms after it
  disconnects. When the last connection drops, any undelivered response lines
  are discarded; the phone re-issues its commands on the next connection.
- **Subscribing.** The phone must write `0x0002` (indications) to the response
  characteristic's CCCD. Notifications (`0x0001`) are not supported; nothing is
  sent until indications are enabled.
- **Commands** must arrive in a single ATT write at offset 0. Long/queued
  writes at a non-zero offset are ignored. Every command in the protocol is
  well under the 20-byte payload of the default 23-byte MTU.

## Framing and MTU

Responses are a newline-delimited ASCII byte stream on the response
characteristic. Each logical line is `ELD <VERB> ...` followed by `\n`.

- A line may span several indications. Each indication carries at most
  `negotiated ATT MTU - 3` bytes (20 bytes until an MTU exchange happens,
  never more than 514).
- The next chunk is sent only after the phone confirms the previous
  indication. If an indication cannot be sent, or is confirmed with an error
  status, the same chunk is retried; the stream never skips or reorders bytes.
- The phone appends each indication to a receive buffer, splits on `\n`, and
  handles each complete line. A trailing partial line waits for the next
  indication.
- On Android the phone requests a 247-byte MTU after connecting (best effort).
  iOS negotiates its own MTU; the chunking above makes the size irrelevant to
  correctness, only to throughput.

Up to 24 response lines and 8 parsed commands are buffered on the device. The
phone serializes commands (one in flight at a time), so these limits are only
reached by a misbehaving client: a full response queue drops the newest line,
a full command queue drops the oldest command.

## Commands

| Command                | Response line(s)                                                     | Phone method (`EldBleTransport`) |
| ---------------------- | -------------------------------------------------------------------- | -------------------------------- |
| `ELD INFO`             | `ELD INFO <json>`                                                    | `readDeviceInfo`                 |
| `ELD LIVE`             | `ELD LIVE <json>`                                                    | `readLiveData`                   |
| `ELD UNSYNCED`         | `ELD UNSYNCED <n>`                                                   | `getUnsyncedCount`               |
| `ELD GET_LATEST`       | `ELD LATEST <seq\|null>`                                             | `getLatestSeq`                   |
| `ELD PULL <from> <to>` | `ELD BATCH from=<from> to=<to> count=<n>`, then `n` × `ELD EVENT <record>`, then `ELD END` | `readRecordsRange` |
| `ELD ACK <from> <to>`  | `ELD ACKED from=<from> to=<to> removed=<n>`                          | `ackSyncedRange`                 |

Details:

- `ELD LATEST?` is accepted as an alias for `ELD GET_LATEST`.
- `ELD LATEST` reports the highest seq still queued on the SD card, falling
  back to the most recently issued seq, or `null` if neither exists.
- `ELD UNSYNCED` is the number of records still queued on the SD card.
- `ELD PULL` returns queued records with `from <= seq <= to`, in queue order,
  **at most 16 per batch** (`PHONE_BATCH_MAX_EVENTS`). `from`/`to` in the
  `BATCH` header echo the request; `count` is what was actually returned.
  Records past the cap stay queued and are returned by a later pull. `PULL`
  never removes anything.
- `ELD ACK <from> <to>` removes every queued record with `from <= seq <= to`
  and reports how many were removed. `ELD ACK <seq>` is shorthand for
  `ELD ACK <seq> <seq>`. The phone only ACKs seqs it pulled in the current
  pass *and* the server accepted or reported as duplicate, and it splits them
  into contiguous ranges (`toAckRanges` in `phone-relay/src/protocol.ts`), so
  a rejected or unread seq in the middle of a range is never ACKed.
- `<from>`/`<to>`/`<seq>` are unsigned decimal integers. Seqs are at most 53
  bits (see `ingest-contract.md`), so they are exact as JavaScript numbers.

### `ELD INFO` payload

```json
{"deviceId":"eld-a1b2c3d4e5f6","firmwareVersion":"0.1.0","vin":null,"sdStatus":"ok","inputVoltage":null}
```

`vin` is `null` until a VIN has been decoded from the bus. `sdStatus` is one of
`ok`, `full`, `missing`, `error`. `inputVoltage` is `null` on the current
hardware, which has no supply-voltage ADC divider.

### `ELD LIVE` payload

```json
{"rpm":612.00,"speedMph":0.00,"fuelPercent":72.00,"engineHours":1234.50,"latitude":33.950000,"longitude":-84.000000,"gpsHdop":0.90,"gpsFixAgeMs":800,"gpsSentences":4210,"faults":["SPN 110 FMI 0"]}
```

Keys are camelCase to match the phone's TypeScript types. Speed is mph (the
firmware converts from km/h). Any value the device does not have is `null`.
`faults` lists active DTCs as `"SPN <n> FMI <n>"`.

### `ELD EVENT` record

Each `ELD EVENT` carries one canonical record, byte-for-byte as it sits in the
SD queue and as the server ingests it (`shared/eld-record.schema.json`):

```json
{"device_id":"eld-a1b2c3d4e5f6","seq":1073741825,"ts_ms":1780000000000,"monotonic_ms":42000,"type":"engine_snapshot","data":{"rpm":612,"speed_mph":0,"fuel_percent":72}}
```

`type` is one of the values the server accepts (`RecordType` in
`ingest-server/src/model.rs`): `engine_snapshot`, `fault_code`, `vin`,
`odometer`, `driver_event`, `heartbeat`, `bus_census`. The firmware maps its
internal `OutboundKind` onto that set (`firmware/src/comms/protocol.rs`):

| Firmware `OutboundKind` | Wire `type`       |
| ----------------------- | ----------------- |
| `TelemetrySnapshot`     | `engine_snapshot` |
| `TripSummary`           | `driver_event`    |
| `DiagnosticReport`      | `heartbeat`       |
| `BusCensus`             | `bus_census`      |
| `Vin`                   | `vin`             |
| `FaultCode`             | `fault_code`      |
| `PendingUploadChunk`    | _(re-ships an already-serialized record; its label is never written)_ |

## Error handling

- **Unparseable command over BLE** (unknown verb, missing or non-numeric
  argument, invalid UTF-8): the write is acknowledged at the ATT layer, the
  device counts a parse error, and no response line is sent. The phone's
  per-command timeout (10 s) rejects the call.
- **Unparseable command over serial:** the device answers
  `ELD ERROR bad_command`, or `ELD ERROR line_too_long` for a line over 128
  bytes.
- **`ELD ERROR <reason>`** on either link rejects the phone's in-flight command
  with `<reason>`.
- **SD read/write failure:** logged on the device. `PULL` returns an empty
  batch, `UNSYNCED` returns `0`, and `ACK` reports `removed=0`, so nothing is
  ever ACKed that was not actually removed.
- **Malformed `ELD EVENT` JSON:** the phone skips that line and keeps the rest
  of the batch. The record stays on the device because it was never uploaded,
  so it is never ACKed.
- **Disconnect mid-command:** the phone rejects every queued command with
  `Disconnected.` and clears its receive buffer; the device clears its
  response queue. Nothing was ACKed, so the next sync re-pulls.

## Phone sync loop

`useEldBleSync.ts` drives one pass:

`IDLE → CONNECTED_TO_PHONE → READING_RECORDS → UPLOADING_FROM_PHONE → ACKING_RECORDS → SYNC_COMPLETE` (or `SYNC_FAILED`).

1. Connect, then `INFO`, `LIVE`, `GET_LATEST` and `UNSYNCED`.
2. If nothing is unsynced, upload anything still pending in the phone's local
   store and finish.
3. Otherwise `PULL` in windows of 50 seqs. Each window's records are saved to
   the phone's durable store, the store's pending records are uploaded, and the
   accepted/duplicate seqs from that window are ACKed back as contiguous
   ranges.
4. Disconnect.

Upload is skipped while the phone is offline; pulled records then stay in the
phone store and on the device until a later pass can upload and ACK them.

## Trying it on hardware

1. Build and flash the firmware with the default `bluetooth` feature:
   `cd firmware && tools/flash.sh`.
2. `react-native-ble-plx` is a native module, so the host React Native app needs
   a development build (not Expo Go) plus the ble-plx config plugin for the iOS
   Bluetooth usage string and Android `BLUETOOTH_SCAN`/`BLUETOOTH_CONNECT`
   permissions.
3. To check the GATT side on its own, use a generic BLE client such as nRF
   Connect: connect to `ELD-DONGLE`, enable indications on the response
   characteristic, and write `ELD INFO` to the command characteristic.
