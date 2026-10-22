import type { EldRecord } from "./storage";

export type EldDeviceInfo = {
  deviceId: string;
  firmwareVersion: string;
  vin?: string;
  sdStatus: "ok" | "full" | "missing" | "error";
  inputVoltage?: number;
  batteryPercent?: number;
};

export type EldLiveData = {
  rpm?: number;
  speedMph?: number;
  fuelPercent?: number;
  engineHours?: number;
  latitude?: number;
  longitude?: number;
  gpsHdop?: number;
  gpsFixAgeMs?: number;
  gpsSentences?: number;
  faults?: string[];
};

export type AckRange = {
  fromSeq: number;
  toSeq: number;
};

export type EldBleTransport = {
  connect: () => Promise<void>;
  disconnect: () => Promise<void>;
  readDeviceInfo: () => Promise<EldDeviceInfo>;
  readLiveData: () => Promise<EldLiveData>;
  getUnsyncedCount: () => Promise<number>;
  getLatestSeq: () => Promise<number | null>;
  readRecordsRange: (fromSeq: number, toSeq: number) => Promise<EldRecord[]>;
  ackSyncedRange: (range: AckRange) => Promise<void>;
};

export type EldSyncState =
  | "IDLE"
  | "CONNECTED_TO_PHONE"
  | "READING_RECORDS"
  | "UPLOADING_FROM_PHONE"
  | "ACKING_RECORDS"
  | "SYNC_COMPLETE"
  | "SYNC_FAILED";

export function toAckRanges(seqs: number[]) {
  const sorted = [...new Set(seqs)].sort((a, b) => a - b);
  const ranges: AckRange[] = [];
  for (const seq of sorted) {
    const last = ranges[ranges.length - 1];
    if (last && last.toSeq + 1 === seq) {
      last.toSeq = seq;
    } else {
      ranges.push({ fromSeq: seq, toSeq: seq });
    }
  }
  return ranges;
}
