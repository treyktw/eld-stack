"""Minimal downstream webhook for the local demo.

POST /events  - one outbox envelope per request, bearer-authenticated.
GET  /stats   - {"received": n, "unique": m}. received > unique would mean the
                outbox redelivered an event (allowed: delivery is at-least-once
                on the wire, and receivers dedupe on event_id).
"""

import hmac
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOKEN = os.environ.get("PIPELINE_TOKEN", "")
received = 0
unique_ids = set()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _reply(self, status, body):
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if self.path == "/stats":
            self._reply(200, {"received": received, "unique": len(unique_ids)})
        else:
            self._reply(404, {"error": "not found"})

    def do_POST(self):
        global received
        if self.path != "/events":
            return self._reply(404, {"error": "not found"})
        auth = self.headers.get("authorization", "")
        if not hmac.compare_digest(auth, f"Bearer {TOKEN}"):
            return self._reply(401, {"error": "unauthorized"})
        body = self.rfile.read(int(self.headers.get("content-length", "0")))
        event = json.loads(body)
        received += 1
        unique_ids.add(event["event_id"])
        print(f"webhook: {event['device_id']} seq={event['seq']} type={event['type']} "
              f"event_id={event['event_id'][:12]}...", flush=True)
        self._reply(200, {"ok": True})


ThreadingHTTPServer(("0.0.0.0", 9000), Handler).serve_forever()
