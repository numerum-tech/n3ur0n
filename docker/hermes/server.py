#!/usr/bin/env python3
"""Hermes — utility functions for the n3ur0n test cluster, over plain HTTP.

These five functions used to be compiled into n3ur0n itself (`UtilityBackend`),
which made every node running that build an identical publisher and gave the
protocol nothing to route. They now live outside the gateway, where a
capability's upstream is supposed to live: a node reaches them through a
`http_base` backend manifest and one cap manifest per function, exactly as it
would reach any third-party API.

Standard library only, one file: this is cluster scaffolding, not a product.
Every endpoint takes a JSON object and returns a JSON object; none of them
touch blob bytes, so the blob layer stays entirely on the n3ur0n side.
"""

import json
import os
import random
import re
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAX_BODY = 64 * 1024


class BadRequest(Exception):
    """A caller error: reported as 400 with a message the node can surface."""


def _require(args, key):
    if key not in args:
        raise BadRequest(f"`{key}` required")
    return args[key]


def _as_text(value):
    """Accept what a planner actually emits: a string, or a number it forgot
    to quote. Anything else is a caller error rather than a silent cast."""
    if isinstance(value, str):
        return value
    if isinstance(value, bool) or value is None:
        raise BadRequest("expected a string")
    if isinstance(value, (int, float)):
        return str(value)
    raise BadRequest("expected a string")


def op_time(_args):
    now = datetime.now(timezone.utc)
    return {"now": now.isoformat().replace("+00:00", "Z"), "unix": int(now.timestamp())}


def op_random_int(args):
    """Bounds are optional, and the bounds actually used come back with the
    draw: a planner that omitted them still sees what range it got."""
    lo = args.get("min", 0)
    hi = args.get("max", 100)
    for label, v in (("min", lo), ("max", hi)):
        if not isinstance(v, int) or isinstance(v, bool):
            raise BadRequest(f"`{label}` must be an integer")
    if lo > hi:
        raise BadRequest("`max` must be >= `min`")
    return {"value": random.randint(lo, hi), "min": lo, "max": hi}


def op_reverse(args):
    text = _as_text(_require(args, "text"))
    return {"reversed": text[::-1]}


def op_string_length(args):
    """Both counts, because they differ: `héllo` is 5 characters and 6 bytes,
    and a caller asking for "length" rarely says which one it meant."""
    text = _as_text(_require(args, "text"))
    return {"chars": len(text), "bytes": len(text.encode("utf-8"))}


def op_rename_file(args):
    """Same bytes, new name. The blob reference travels as data; Hermes never
    fetches the content it describes, so no ticket and no store access."""
    ref = _require(args, "file")
    if not isinstance(ref, dict):
        raise BadRequest("`file` must be a blob reference object")
    digest = ref.get("hash")
    if not isinstance(digest, str) or not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
        raise BadRequest("`file.hash` must be a sha256: digest")
    size = ref.get("size")
    if not isinstance(size, int) or isinstance(size, bool) or size < 1:
        raise BadRequest("`file.size` must be a positive integer")
    new_name = _as_text(_require(args, "new_name")).strip()
    if not new_name:
        raise BadRequest("`new_name` must not be empty")
    return {
        "file": {
            "hash": digest,
            "size": size,
            "mime": ref.get("mime") or "application/octet-stream",
            "name": new_name,
        }
    }


ROUTES = {
    "/time": op_time,
    "/random_int": op_random_int,
    "/reverse": op_reverse,
    "/string_length": op_string_length,
    "/rename_file": op_rename_file,
}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _reply(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self._reply(200, {"status": "ok", "functions": sorted(ROUTES)})
        else:
            self._reply(405, {"error": "use POST"})

    def do_POST(self):
        op = ROUTES.get(self.path)
        if op is None:
            self._reply(404, {"error": f"unknown function {self.path}"})
            return
        length = int(self.headers.get("Content-Length") or 0)
        if length > MAX_BODY:
            self._reply(413, {"error": "body too large"})
            return
        raw = self.rfile.read(length) if length else b"{}"
        try:
            args = json.loads(raw or b"{}")
        except json.JSONDecodeError as e:
            self._reply(400, {"error": f"invalid JSON: {e}"})
            return
        if not isinstance(args, dict):
            self._reply(400, {"error": "body must be a JSON object"})
            return
        try:
            self._reply(200, op(args))
        except BadRequest as e:
            self._reply(400, {"error": str(e)})

    def log_message(self, fmt, *a):
        # One line per call, on stdout: this is the only trace a cluster test
        # has that a capability really left the node.
        print(f"hermes {self.address_string()} {fmt % a}", flush=True)


if __name__ == "__main__":
    port = int(os.environ.get("HERMES_PORT", "8080"))
    print(f"hermes listening on :{port} · {', '.join(sorted(ROUTES))}", flush=True)
    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()
