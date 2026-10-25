"""Shared JSON configuration and gate waits for deterministic provider fixtures."""

import json
from functools import partial
import time
from pathlib import Path
from typing import Any
from mock_files import (
    atomic_write_text, atomic_write_json, atomic_copy, append_record,
    append_jsonl_record, record_lock, CompleteLines, AppendStream,
)

# Ephemeral mock state needs atomic visibility across processes, not power-loss
# durability. Acknowledgement always follows publication. Never retry an append.
publish_text = partial(atomic_write_text, durable=False)
publish_json = partial(atomic_write_json, durable=False)
publish_copy = partial(atomic_copy, durable=False)
append_jsonl = partial(append_jsonl_record, durable=False)
# Input evidence retains three copies (body, rendered text, native input).
# The 4 MiB joined body can expand to 24 MiB per copy under JSON escaping;
# attachments and envelopes must fit too. Only these streams opt into this cap.
INPUT_RECORD_LIMIT = 128 * 1024 * 1024


def append_line(path, text, **options):
    append_record(path, text.encode('utf-8'), durable=False, **options)


def claim_counter(path, limit):
    """Claim one index across fixture processes without losing increments."""
    path = Path(path)
    with record_lock(path):
        current = int(path.read_text()) if path.exists() else 0
        if current >= limit:
            return None
        publish_text(path, str(current + 1))
        return current


def decode_scenario(text: str) -> Any:
    return json.loads(text)


def load_scenario(path: str | Path, *, required: bool = False) -> Any:
    path = Path(path)
    return decode_scenario(path.read_text()) if required or path.exists() else {}


def wait_for_gate(
    path: str | Path | None,
    *,
    timeout: float | None = None,
    timeout_error: Exception | None = None,
    strict_deadline: bool = False,
) -> bool:
    """Wait for a release file, preserving each fixture's timeout policy."""
    gate = Path(path) if path is not None else None
    deadline = time.monotonic() + timeout if timeout is not None else None
    while gate is None or not gate.exists():
        if deadline is not None:
            now = time.monotonic()
            expired = now > deadline if strict_deadline else now >= deadline
            if expired:
                if timeout_error is not None:
                    raise timeout_error
                return False
        time.sleep(0.01)
    return True


def run_stdio_mcp(server: dict) -> str:
    """Exercise a configured stdio child, including initialization and tool RPC."""
    import os
    import subprocess

    frames = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "mock-provider", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "echo", "arguments": {"text": "configured stdio works"}}},
    ]
    result = subprocess.run([server["command"], *server.get("args", [])], input="".join(json.dumps(f) + "\n" for f in frames), text=True, capture_output=True, env={**os.environ, **server.get("env", {})}, timeout=5, check=True)
    replies = {f["id"]: f["result"] for f in map(json.loads, result.stdout.splitlines())}
    assert replies[1]["protocolVersion"] == "2025-11-25"
    assert replies[2]["tools"][0]["name"] == "echo"
    return replies[3]["content"][0]["text"]


def schema_probe(argv):
    """Implement Codex's offline metadata interface, without a test bypass in Hive."""
    if "generate-json-schema" not in argv:
        return False
    if "--experimental" not in argv or "--out" not in argv:
        raise ValueError("schema probe requires experimental output directory")
    output = Path(argv[argv.index("--out") + 1])
    publish_text(output / "ClientRequest.json", Path(__file__).with_name("codex-schema.json").read_text())
    return True
