import { describe, expect, it } from "vitest";
import { toAckRanges } from "./protocol";
import { createMockEldBleTransport } from "./mockTransport";
import { createInMemoryEldRecordStore } from "./storage";

describe("toAckRanges", () => {
  it("returns no ranges for no seqs", () => {
    expect(toAckRanges([])).toEqual([]);
  });

  it("collapses a contiguous run into one range", () => {
    expect(toAckRanges([10241, 10242, 10243])).toEqual([
      { fromSeq: 10241, toSeq: 10243 },
    ]);
  });

  it("splits at gaps so a rejected seq in the middle is never ACKed", () => {
    // 3 was rejected by the server (or failed upload): the ELD must keep it.
    expect(toAckRanges([1, 2, 4, 5, 7])).toEqual([
      { fromSeq: 1, toSeq: 2 },
      { fromSeq: 4, toSeq: 5 },
      { fromSeq: 7, toSeq: 7 },
    ]);
  });

  it("sorts and dedupes, since accepted + duplicate lists can overlap and arrive unordered", () => {
    expect(toAckRanges([5, 3, 4, 3, 1, 5])).toEqual([
      { fromSeq: 1, toSeq: 1 },
      { fromSeq: 3, toSeq: 5 },
    ]);
  });

  it("does not mutate its input", () => {
    const seqs = [3, 1, 2];
    toAckRanges(seqs);
    expect(seqs).toEqual([3, 1, 2]);
  });
});

describe("mock transport ACK semantics", () => {
  it("refuses commands before connect", async () => {
    const transport = createMockEldBleTransport();
    await expect(transport.getUnsyncedCount()).rejects.toThrow(/not connected/);
  });

  it("drops only ACKed ranges, leaving gaps queued on the device", async () => {
    const transport = createMockEldBleTransport();
    await transport.connect();

    const latest = await transport.getLatestSeq();
    const total = await transport.getUnsyncedCount();
    expect(latest).not.toBeNull();
    const first = latest! - total + 1;

    const records = await transport.readRecordsRange(first, latest!);
    expect(records).toHaveLength(total);

    // Pretend the server accepted everything except the third record.
    const lost = records[2]!.seq;
    const synced = records.map((r) => r.seq).filter((seq) => seq !== lost);
    for (const range of toAckRanges(synced)) {
      await transport.ackSyncedRange(range);
    }

    expect(await transport.getUnsyncedCount()).toBe(1);
    const remaining = await transport.readRecordsRange(first, latest!);
    expect(remaining.map((r) => r.seq)).toEqual([lost]);
  });
});

describe("in-memory store", () => {
  it("is idempotent on (device_id, seq) so a re-pull cannot resurrect an uploaded record", async () => {
    const store = createInMemoryEldRecordStore();
    const record = {
      device_id: "eld_mock_001",
      seq: 1,
      ts_ms: null,
      type: "heartbeat" as const,
      data: {},
    };
    await store.saveRecords([record]);
    await store.markUploaded("eld_mock_001", [1]);
    await store.saveRecords([record]);
    expect(await store.getCounts()).toEqual({ pending: 0, uploaded: 1, rejected: 0 });
  });
});
