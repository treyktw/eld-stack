import { useCallback, useMemo, useRef, useState } from "react";
import type { EldBatchUploader, EldRecord, EldRecordStore } from "./storage";
import {
  toAckRanges,
  type EldBleTransport,
  type EldDeviceInfo,
  type EldLiveData,
  type EldSyncState,
} from "./protocol";

const BLE_READ_CHUNK = 50;
const LOCAL_UPLOAD_CHUNK = 50;

type SyncStats = {
  pulled: number;
  uploaded: number;
  duplicate: number;
  rejected: number;
  pendingLocal: number;
};

export type UseEldBleSyncInput = {
  /** BLE transport: `createBleEldTransport()` on a device, the mock elsewhere. */
  transport: EldBleTransport;
  /** Durable phone-side store; records live here between pull and upload. */
  store: EldRecordStore;
  /** Sends a batch to the backend; the host app owns auth and endpoint. */
  uploadBatch: EldBatchUploader;
  /** Network reachability. Offline sync still pulls; it just cannot ACK. */
  isOnline: boolean;
  /** False while the store is still opening. Defaults to true. */
  storeReady?: boolean;
  /** Feature kill-switch for pairing/sync. Defaults to true. */
  enabled?: boolean;
};

export function useEldBleSync(input: UseEldBleSyncInput) {
  const { isOnline, store, uploadBatch } = input;
  const localDbReady = input.storeReady ?? true;
  const eldDeviceEnabled = input.enabled ?? true;
  const transportRef = useRef<EldBleTransport | null>(null);
  transportRef.current ??= input.transport;
  const [state, setState] = useState<EldSyncState>("IDLE");
  const [deviceInfo, setDeviceInfo] = useState<EldDeviceInfo | null>(null);
  const [liveData, setLiveData] = useState<EldLiveData | null>(null);
  const [stats, setStats] = useState<SyncStats>({
    pulled: 0,
    uploaded: 0,
    duplicate: 0,
    rejected: 0,
    pendingLocal: 0,
  });
  const [error, setError] = useState<string | null>(null);
  const syncingRef = useRef(false);

  const canUpload = isOnline;

  const uploadLocalPending = useCallback(
    async (deviceId: string) => {
      if (!isOnline) {
        const counts = await store.getCounts();
        setStats((current) => ({ ...current, pendingLocal: counts.pending }));
        return { uploaded: 0, duplicate: 0, rejected: 0, ackedSeqs: [] };
      }

      let uploaded = 0;
      let duplicate = 0;
      let rejected = 0;
      const ackedSeqs: number[] = [];

      while (true) {
        const pending = await store.listPending(LOCAL_UPLOAD_CHUNK);
        if (pending.length === 0) break;
        const records = pending.map(
          (record: EldRecord): EldRecord => ({
            device_id: record.device_id,
            seq: record.seq,
            ts_ms: record.ts_ms,
            monotonic_ms: record.monotonic_ms,
            type: record.type,
            data: record.data,
          }),
        );

        const response = await uploadBatch({ deviceId, records });

        const accepted = response.accepted ?? [];
        const duplicates = response.duplicate ?? [];
        const rejectedItems = response.rejected ?? [];

        await store.markUploaded(deviceId, [...accepted, ...duplicates]);
        await store.markRejected(
          deviceId,
          rejectedItems.map((item) => ({
            seq: item.seq,
            reason: item.reason,
          })),
        );

        uploaded += accepted.length;
        duplicate += duplicates.length;
        rejected += rejectedItems.length;
        ackedSeqs.push(...accepted, ...duplicates);

        if (accepted.length + duplicates.length + rejectedItems.length === 0) {
          break;
        }
      }

      const counts = await store.getCounts();
      setStats((current) => ({
        ...current,
        uploaded: current.uploaded + uploaded,
        duplicate: current.duplicate + duplicate,
        rejected: current.rejected + rejected,
        pendingLocal: counts.pending,
      }));
      return { uploaded, duplicate, rejected, ackedSeqs };
    },
    [isOnline, store, uploadBatch],
  );

  const syncOnce = useCallback(async () => {
    if (!eldDeviceEnabled) {
      setError("ELD device sync is not available yet.");
      setState("IDLE");
      return;
    }
    if (syncingRef.current || !localDbReady) return;
    syncingRef.current = true;
    setError(null);
    try {
      const transport = transportRef.current;
      if (!transport) throw new Error("ELD BLE transport is not configured.");

      await transport.connect();
      setState("CONNECTED_TO_PHONE");

      const info = await transport.readDeviceInfo();
      setDeviceInfo(info);
      setLiveData(await transport.readLiveData());

      const latestSeq = await transport.getLatestSeq();
      const unsyncedCount = await transport.getUnsyncedCount();
      if (!latestSeq || unsyncedCount === 0) {
        await uploadLocalPending(info.deviceId);
        setState("SYNC_COMPLETE");
        return;
      }

      let pulled = 0;
      let cursor = Math.max(1, latestSeq - unsyncedCount + 1);
      while (cursor <= latestSeq) {
        setState("READING_RECORDS");
        const toSeq = Math.min(latestSeq, cursor + BLE_READ_CHUNK - 1);
        const records = await transport.readRecordsRange(cursor, toSeq);
        await store.saveRecords(records);
        pulled += records.length;
        setStats((current) => ({
          ...current,
          pulled: current.pulled + records.length,
        }));

        setState("UPLOADING_FROM_PHONE");
        const result = await uploadLocalPending(info.deviceId);

        if (canUpload && result.ackedSeqs.length > 0) {
          const justRead = new Set(records.map((record) => record.seq));
          const ackableLocal = result.ackedSeqs.filter((seq) =>
            justRead.has(seq),
          );
          setState("ACKING_RECORDS");
          for (const range of toAckRanges(ackableLocal)) {
            await transport.ackSyncedRange(range);
          }
        }

        cursor = toSeq + 1;
      }

      const counts = await store.getCounts();
      setStats((current) => ({
        ...current,
        pulled,
        pendingLocal: counts.pending,
      }));
      setState("SYNC_COMPLETE");
    } catch (e) {
      setState("SYNC_FAILED");
      setError(e instanceof Error ? e.message : "ELD sync failed.");
    } finally {
      try {
        await transportRef.current?.disconnect();
      } catch {
        /* ignore disconnect failures */
      }
      syncingRef.current = false;
    }
  }, [canUpload, eldDeviceEnabled, localDbReady, store, uploadLocalPending]);

  return useMemo(
    () => ({
      state,
      deviceInfo,
      liveData,
      stats,
      error,
      canUpload,
      syncOnce,
    }),
    [canUpload, deviceInfo, error, liveData, state, stats, syncOnce],
  );
}
