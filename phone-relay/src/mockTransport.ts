import type { EldBleTransport, EldDeviceInfo, EldLiveData } from "./protocol";
import type { EldRecord } from "./storage";

const DEVICE_ID = "eld_mock_001";

function makeRecords(): EldRecord[] {
  const base = Date.now() - 15 * 60_000;
  return Array.from({ length: 12 }, (_, i) => ({
    device_id: DEVICE_ID,
    seq: 10241 + i,
    ts_ms: base + i * 30_000,
    type: "engine_snapshot",
    data: {
      rpm: i % 4 === 0 ? 612 : 1240 + i * 12,
      speed_mph: i % 4 === 0 ? 0 : 54 + (i % 5),
      fuel_percent: 72 - i,
      lat: 39.8283,
      lon: -98.5795,
    },
  }));
}

export function createMockEldBleTransport(): EldBleTransport {
  const records = makeRecords();
  const acked = new Set<number>();
  let connected = false;

  const assertConnected = () => {
    if (!connected) throw new Error("ELD transport is not connected.");
  };

  return {
    async connect() {
      connected = true;
    },
    async disconnect() {
      connected = false;
    },
    async readDeviceInfo(): Promise<EldDeviceInfo> {
      assertConnected();
      return {
        deviceId: DEVICE_ID,
        firmwareVersion: "mock-0.1.0",
        vin: "MOCKVIN0000000001",
        sdStatus: "ok",
        inputVoltage: 13.8,
      };
    },
    async readLiveData(): Promise<EldLiveData> {
      assertConnected();
      return {
        rpm: 612,
        speedMph: 0,
        fuelPercent: 72,
        engineHours: 1842.4,
        latitude: 39.8283,
        longitude: -98.5795,
        gpsHdop: 0.9,
        gpsFixAgeMs: 250,
        gpsSentences: 42,
        faults: [],
      };
    },
    async getUnsyncedCount() {
      assertConnected();
      return records.filter((record) => !acked.has(record.seq)).length;
    },
    async getLatestSeq() {
      assertConnected();
      return records.at(-1)?.seq ?? null;
    },
    async readRecordsRange(fromSeq: number, toSeq: number) {
      assertConnected();
      return records.filter(
        (record) =>
          record.seq >= fromSeq && record.seq <= toSeq && !acked.has(record.seq),
      );
    },
    async ackSyncedRange(range) {
      assertConnected();
      for (let seq = range.fromSeq; seq <= range.toSeq; seq += 1) {
        acked.add(seq);
      }
    },
  };
}
