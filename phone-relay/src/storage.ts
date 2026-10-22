// Phone-side record storage.
//
// The phone is a disposable transport: records pulled over BLE are written
// here first, uploaded from here, and only then ACKed back to the ELD. Keeping
// the store behind an interface lets the host app back it with SQLite, MMKV,
// or anything else durable. `createInMemoryEldRecordStore` is for tests and
// demos only - it does not survive an app restart, which is exactly the
// property a real store must have.

/** Record types accepted by the ingest boundary (see docs/ingest-contract.md). */
export type EldRecordType =
  | "engine_snapshot"
  | "fault_code"
  | "vin"
  | "odometer"
  | "driver_event"
  | "heartbeat"
  | "bus_census";

/** Canonical record shape, identical on the firmware, the phone and the server. */
export type EldRecord = {
  device_id: string;
  seq: number;
  ts_ms: number | null;
  monotonic_ms?: number | null;
  type: EldRecordType;
  data: Record<string, unknown>;
};

export type EldRecordCounts = {
  pending: number;
  uploaded: number;
  rejected: number;
};

export type EldRejection = {
  seq: number;
  reason: string;
};

export interface EldRecordStore {
  /** Persist records pulled from the device. Must be idempotent on (device_id, seq). */
  saveRecords(records: EldRecord[]): Promise<void>;
  /** Oldest-first records that have not been uploaded or rejected yet. */
  listPending(limit: number): Promise<EldRecord[]>;
  markUploaded(deviceId: string, seqs: number[]): Promise<void>;
  markRejected(deviceId: string, rejections: EldRejection[]): Promise<void>;
  getCounts(): Promise<EldRecordCounts>;
}

type StoredRecord = {
  record: EldRecord;
  status: "pending" | "uploaded" | "rejected";
  reason?: string;
};

const keyOf = (deviceId: string, seq: number) => `${deviceId}:${seq}`;

export function createInMemoryEldRecordStore(): EldRecordStore {
  const rows = new Map<string, StoredRecord>();

  return {
    async saveRecords(records) {
      for (const record of records) {
        const key = keyOf(record.device_id, record.seq);
        // A re-pull of an already-stored record must not reset its status:
        // the ELD re-sends anything it has not seen ACKed.
        if (!rows.has(key)) rows.set(key, { record, status: "pending" });
      }
    },
    async listPending(limit) {
      return [...rows.values()]
        .filter((row) => row.status === "pending")
        .sort((a, b) => a.record.seq - b.record.seq)
        .slice(0, limit)
        .map((row) => row.record);
    },
    async markUploaded(deviceId, seqs) {
      for (const seq of seqs) {
        const row = rows.get(keyOf(deviceId, seq));
        if (row) row.status = "uploaded";
      }
    },
    async markRejected(deviceId, rejections) {
      for (const { seq, reason } of rejections) {
        const row = rows.get(keyOf(deviceId, seq));
        if (row) {
          row.status = "rejected";
          row.reason = reason;
        }
      }
    },
    async getCounts() {
      const counts: EldRecordCounts = { pending: 0, uploaded: 0, rejected: 0 };
      for (const row of rows.values()) counts[row.status] += 1;
      return counts;
    },
  };
}

/** Per-seq result of one upload, so the phone knows exactly which seqs to ACK. */
export type EldUploadResult = {
  accepted: number[];
  duplicate: number[];
  rejected: EldRejection[];
};

/**
 * Sends a batch of records to the backend. Implemented by the host app: it
 * owns authentication and the endpoint. Accepted *and* duplicate seqs are both
 * safe to ACK back to the ELD - a duplicate means the record is already stored.
 */
export type EldBatchUploader = (batch: {
  deviceId: string;
  records: EldRecord[];
}) => Promise<EldUploadResult>;
