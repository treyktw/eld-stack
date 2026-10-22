#!/usr/bin/env python3
"""Decode captured ELD firmware logs into a readable table.

Parses the firmware's compact line format emitted over serial / written to SD:

    DATA rpm 1450.0 src=0x00
    DATA speed_kph 88.5 src=0x00
    DATA fuel_level_pct 73.0 src=0x17
    DATA engine_hours 4123.5 src=0x00
    REQ 0x00FEE0
    heartbeat | bus=250k uptime=42s rx=1203 ...

Usage:
    tools/decode-log.py serial.log        # from a file
    cat serial.log | tools/decode-log.py  # from stdin
    tools/decode-log.py --json log.txt    # emit JSONL instead of a table
"""
import argparse
import json
import re
import sys

DATA_RE = re.compile(r"DATA\s+(\S+)\s+(\S+)(?:\s+src=(\S+))?")
REQ_RE = re.compile(r"REQ\s+(\S+)")
HEARTBEAT_RE = re.compile(r"heartbeat\s*\|\s*(.*)")
DIAG_RE = re.compile(r"\bDIAG\s+(.*)$")

# SD record format (storage/schema.rs): "<ts_ms> <TYPE> key=val key=val ..."
SD_TYPES = ("BOOT", "SIGNAL", "HEARTBEAT", "TRIP", "MILEAGE", "POWER", "UPLOAD", "ERROR")
SD_RE = re.compile(r"^(\d+)\s+(" + "|".join(SD_TYPES) + r")\s*(.*)$")


def parse_line(line: str):
    line = line.strip()
    if not line:
        return None
    # SD record lines (from files on the card) take priority — they start with
    # a timestamp and an uppercase record type.
    if (m := SD_RE.match(line)):
        ts, rtype, rest = int(m.group(1)), m.group(2), m.group(3)
        fields = {}
        for tok in rest.split():
            if "=" in tok:
                k, v = tok.split("=", 1)
                fields[k] = v
        return {"kind": "record", "ts_ms": ts, "type": rtype, "fields": fields}
    if (m := DIAG_RE.search(line)):
        fields = {}
        for tok in m.group(1).split():
            if "=" in tok:
                k, v = tok.split("=", 1)
                fields[k] = v
        return {"kind": "diag", "fields": fields}
    if (m := DATA_RE.search(line)):
        signal, value, src = m.group(1), m.group(2), m.group(3)
        try:
            value = float(value)
        except ValueError:
            pass
        return {"kind": "data", "signal": signal, "value": value, "src": src}
    if (m := REQ_RE.search(line)):
        return {"kind": "req", "pgn": m.group(1)}
    if (m := HEARTBEAT_RE.search(line)):
        fields = {}
        for tok in m.group(1).split():
            if "=" in tok:
                k, v = tok.split("=", 1)
                fields[k] = v
        return {"kind": "heartbeat", "fields": fields}
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description="Decode ELD firmware logs.")
    ap.add_argument("file", nargs="?", help="log file (default: stdin)")
    ap.add_argument("--json", action="store_true", help="emit JSONL")
    args = ap.parse_args()

    stream = open(args.file, encoding="utf-8", errors="replace") if args.file else sys.stdin

    for line in stream:
        rec = parse_line(line)
        if rec is None:
            continue
        if args.json:
            print(json.dumps(rec))
            continue
        if rec["kind"] == "record":
            kv = "  ".join(f"{k}={v}" for k, v in rec["fields"].items())
            print(f"[{rec['ts_ms']:>10}] {rec['type']:<9} {kv}")
        elif rec["kind"] == "diag":
            kv = "  ".join(f"{k}={v}" for k, v in rec["fields"].items())
            print(f"== DIAG: {kv}")
        elif rec["kind"] == "data":
            src = f"  src={rec['src']}" if rec["src"] else ""
            print(f"  {rec['signal']:<18} = {rec['value']}{src}")
        elif rec["kind"] == "req":
            print(f"  REQUEST PGN {rec['pgn']}")
        elif rec["kind"] == "heartbeat":
            kv = "  ".join(f"{k}={v}" for k, v in rec["fields"].items())
            print(f"-- heartbeat: {kv}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
