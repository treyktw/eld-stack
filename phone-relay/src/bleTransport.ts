import { PermissionsAndroid, Platform } from "react-native";
import { BleManager, type Device } from "react-native-ble-plx";
import type { EldRecord, EldRecordType } from "./storage";
import type {
  AckRange,
  EldBleTransport,
  EldDeviceInfo,
  EldLiveData,
} from "./protocol";

// GATT contract — must match the firmware (firmware/src/comms/bluetooth.rs).
const SERVICE_UUID = "fc59c4f2-bb03-4328-8361-267acb7e0842";
const COMMAND_UUID = "fc59c4f3-bb03-4328-8361-267acb7e0842";
const RESPONSE_UUID = "fc59c4f4-bb03-4328-8361-267acb7e0842";
// The firmware advertises the service UUID in the primary packet so iOS can
// discover and reconnect in the background. Its name lives in scan response,
// but UUID matching is the stable identity used here.

const SCAN_TIMEOUT_MS = 12_000;
const COMMAND_TIMEOUT_MS = 10_000;
const ANDROID_MTU = 247;

// ---- base64 (responses are ASCII; btoa/atob are available in Hermes) -------
const encode = (text: string) => globalThis.btoa(text);
const decode = (b64: string) => globalThis.atob(b64);

type Pending = {
  command: string;
  /** Returns true once `lines` form a complete response. */
  isComplete: (lines: string[]) => boolean;
  resolve: (lines: string[]) => void;
  reject: (error: Error) => void;
  lines: string[];
  timer: ReturnType<typeof setTimeout> | null;
};

/**
 * Real BLE transport that speaks the firmware's line protocol over the command
 * (write) and response (indicate) characteristics. Commands are serialized: one
 * is in flight at a time, its response lines accumulate until complete.
 */
export function createBleEldTransport(): EldBleTransport {
  let manager: BleManager | null = null;
  let device: Device | null = null;
  let monitorRemove: (() => void) | null = null;
  let receiveBuffer = "";
  const queue: Pending[] = [];

  const getManager = () => {
    if (!manager) {
      manager = new BleManager();
    }
    return manager;
  };

  function settleHead(ok: boolean, error?: Error) {
    const head = queue[0];
    if (!head) return;
    if (head.timer) clearTimeout(head.timer);
    queue.shift();
    if (ok) head.resolve(head.lines);
    else head.reject(error ?? new Error("ELD command failed"));
    pump();
  }

  function onLine(line: string) {
    const head = queue[0];
    if (!head) return;
    if (line.startsWith("ELD ERROR")) {
      settleHead(false, new Error(line.slice(4).trim()));
      return;
    }
    head.lines.push(line);
    if (head.isComplete(head.lines)) settleHead(true);
  }

  function onIndication(value: string | null) {
    if (!value) return;
    // ATT MTU differs by phone, so firmware fragments logical lines across
    // confirmed indications. Reassemble the newline-delimited stream here.
    receiveBuffer += decode(value);
    const parts = receiveBuffer.split("\n");
    receiveBuffer = parts.pop() ?? "";
    for (const raw of parts) {
      const line = raw.trim();
      if (line) onLine(line);
    }
  }

  async function pump() {
    const head = queue[0];
    if (!head || !device) return;
    head.timer = setTimeout(() => {
      settleHead(false, new Error(`ELD command timed out: ${head.command}`));
    }, COMMAND_TIMEOUT_MS);
    try {
      await device.writeCharacteristicWithResponseForService(
        SERVICE_UUID,
        COMMAND_UUID,
        encode(head.command),
      );
    } catch (e) {
      settleHead(false, e instanceof Error ? e : new Error(String(e)));
    }
  }

  function run(command: string, isComplete: Pending["isComplete"]) {
    return new Promise<string[]>((resolve, reject) => {
      const wasIdle = queue.length === 0;
      queue.push({
        command,
        isComplete,
        resolve,
        reject,
        lines: [],
        timer: null,
      });
      if (wasIdle) void pump();
    });
  }

  /** A response that is exactly one line beginning with `prefix`. */
  const single = (prefix: string) => (lines: string[]) =>
    lines.some((l) => l.startsWith(prefix));

  async function ensureAndroidPermissions() {
    if (Platform.OS !== "android") return;
    const granted = await PermissionsAndroid.requestMultiple([
      PermissionsAndroid.PERMISSIONS.BLUETOOTH_SCAN,
      PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT,
      // Older Android (<12) gates BLE scanning behind fine location.
      PermissionsAndroid.PERMISSIONS.ACCESS_FINE_LOCATION,
    ]);
    const denied = Object.values(granted).some(
      (v) => v !== PermissionsAndroid.RESULTS.GRANTED,
    );
    if (denied) throw new Error("Bluetooth permission denied.");
  }

  function waitForPoweredOn(mgr: BleManager) {
    return new Promise<void>((resolve, reject) => {
      const sub = mgr.onStateChange((state) => {
        if (state === "PoweredOn") {
          sub.remove();
          resolve();
        } else if (state === "PoweredOff" || state === "Unauthorized") {
          sub.remove();
          reject(new Error(`Bluetooth unavailable (${state}).`));
        }
      }, true);
    });
  }

  function scanForDevice(mgr: BleManager) {
    return new Promise<Device>((resolve, reject) => {
      const timer = setTimeout(() => {
        mgr.stopDeviceScan();
        reject(new Error("No ELD device found nearby."));
      }, SCAN_TIMEOUT_MS);

      mgr.startDeviceScan([SERVICE_UUID], { allowDuplicates: false }, (error, found) => {
        if (error) {
          clearTimeout(timer);
          mgr.stopDeviceScan();
          reject(error);
          return;
        }
        if (found) {
          clearTimeout(timer);
          mgr.stopDeviceScan();
          resolve(found);
        }
      });
    });
  }

  return {
    async connect() {
      const mgr = getManager();
      await ensureAndroidPermissions();
      await waitForPoweredOn(mgr);

      let dev = await scanForDevice(mgr);
      dev = await dev.connect();
      dev = await dev.discoverAllServicesAndCharacteristics();
      if (Platform.OS === "android") {
        try {
          dev = await dev.requestMTU(ANDROID_MTU);
        } catch {
          /* MTU bump is best-effort; fall back to the negotiated default. */
        }
      }

      const sub = dev.monitorCharacteristicForService(
        SERVICE_UUID,
        RESPONSE_UUID,
        (error, characteristic) => {
          if (error) return;
          onIndication(characteristic?.value ?? null);
        },
      );
      monitorRemove = () => sub.remove();
      device = dev;
    },

    async disconnect() {
      monitorRemove?.();
      monitorRemove = null;
      receiveBuffer = "";
      const dev = device;
      device = null;
      // Fail any in-flight command so callers don't hang.
      while (queue.length) settleHead(false, new Error("Disconnected."));
      if (dev) {
        try {
          await dev.cancelConnection();
        } catch {
          /* already gone */
        }
      }
    },

    async readDeviceInfo(): Promise<EldDeviceInfo> {
      const lines = await run("ELD INFO", single("ELD INFO "));
      const line = lines.find((l) => l.startsWith("ELD INFO "));
      if (!line) {
        throw new Error("ELD INFO response missing.");
      }
      const raw = JSON.parse(line.slice("ELD INFO ".length)) as {
        deviceId: string;
        firmwareVersion: string;
        vin?: string | null;
        sdStatus: EldDeviceInfo["sdStatus"];
        inputVoltage?: number | null;
        batteryPercent?: number | null;
      };
      return {
        deviceId: raw.deviceId,
        firmwareVersion: raw.firmwareVersion,
        vin: raw.vin ?? undefined,
        sdStatus: raw.sdStatus,
        inputVoltage: raw.inputVoltage ?? undefined,
        batteryPercent: raw.batteryPercent ?? undefined,
      };
    },

    async readLiveData(): Promise<EldLiveData> {
      const lines = await run("ELD LIVE", single("ELD LIVE "));
      const line = lines.find((l) => l.startsWith("ELD LIVE "));
      if (!line) {
        throw new Error("ELD LIVE response missing.");
      }
      const raw = JSON.parse(line.slice("ELD LIVE ".length)) as {
        rpm?: number | null;
        speedMph?: number | null;
        fuelPercent?: number | null;
        engineHours?: number | null;
        latitude?: number | null;
        longitude?: number | null;
        gpsHdop?: number | null;
        gpsFixAgeMs?: number | null;
        gpsSentences?: number | null;
        faults?: string[] | null;
      };
      return {
        rpm: raw.rpm ?? undefined,
        speedMph: raw.speedMph ?? undefined,
        fuelPercent: raw.fuelPercent ?? undefined,
        engineHours: raw.engineHours ?? undefined,
        latitude: raw.latitude ?? undefined,
        longitude: raw.longitude ?? undefined,
        gpsHdop: raw.gpsHdop ?? undefined,
        gpsFixAgeMs: raw.gpsFixAgeMs ?? undefined,
        gpsSentences: raw.gpsSentences ?? undefined,
        faults: raw.faults ?? [],
      };
    },

    async getUnsyncedCount(): Promise<number> {
      const lines = await run("ELD UNSYNCED", single("ELD UNSYNCED "));
      const line = lines.find((l) => l.startsWith("ELD UNSYNCED "));
      if (!line) {
        throw new Error("ELD UNSYNCED response missing.");
      }
      const n = Number.parseInt(line.slice("ELD UNSYNCED ".length).trim(), 10);
      return Number.isFinite(n) ? n : 0;
    },

    async getLatestSeq(): Promise<number | null> {
      const lines = await run("ELD GET_LATEST", single("ELD LATEST "));
      const line = lines.find((l) => l.startsWith("ELD LATEST "));
      if (!line) {
        throw new Error("ELD LATEST response missing.");
      }
      const value = line.slice("ELD LATEST ".length).trim();
      if (value === "null") return null;
      const n = Number.parseInt(value, 10);
      return Number.isFinite(n) ? n : null;
    },

    async readRecordsRange(fromSeq: number, toSeq: number): Promise<EldRecord[]> {
      const lines = await run(
        `ELD PULL ${fromSeq} ${toSeq}`,
        (ls) => ls.some((l) => l === "ELD END"),
      );
      const records: EldRecord[] = [];
      for (const line of lines) {
        if (!line.startsWith("ELD EVENT ")) continue;
        try {
          const obj = JSON.parse(line.slice("ELD EVENT ".length)) as {
            device_id: string;
            seq: number;
            ts_ms: number | null;
            monotonic_ms?: number | null;
            type: string;
            data: Record<string, unknown>;
          };
          records.push({
            device_id: obj.device_id,
            seq: obj.seq,
            ts_ms: obj.ts_ms,
            monotonic_ms: obj.monotonic_ms ?? null,
            type: obj.type as EldRecordType,
            data: obj.data ?? {},
          });
        } catch {
          /* skip a malformed event line rather than failing the whole batch */
        }
      }
      return records;
    },

    async ackSyncedRange(range: AckRange): Promise<void> {
      await run(
        `ELD ACK ${range.fromSeq} ${range.toSeq}`,
        single("ELD ACKED "),
      );
    },
  };
}
