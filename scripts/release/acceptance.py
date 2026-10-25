"""Small acceptance helpers for installed release archives and deterministic providers."""
import errno
import json
import os
import pty
import select
import subprocess
import time
from collections.abc import Callable
from typing import Any
from .process import json_reply
from .progress import cleanup_all, finalizer


def require(condition, detail):
    if not condition:
        raise RuntimeError(detail)


class Cli:
    def __init__(self, command: list[str], *, timeout: int = 45, prefixes: dict[str, list[str]] | None = None):
        self.command = command
        self.prefixes = prefixes or {}
        self.timeout = timeout

    def argv(self, *arguments: str) -> list[str]:
        """Choose an explicitly configured command-family prefix."""
        return self.prefixes.get(arguments[0], self.command) + list(arguments)

    def __call__(self, *arguments: str) -> dict[str, Any]:
        result = subprocess.run(
            self.argv(*arguments), capture_output=True, text=True, timeout=self.timeout
        , check=False)
        return json_reply(result)


def event_pages(call: Callable, agent: str, *, limit: int = 200) -> list[dict[str, Any]]:
    """Read complete durable history and reject expired or nonadvancing pages."""
    events, after = [], 0
    while True:
        page = call("read", agent, "--after", str(after), "--limit", str(limit))
        require(not page.get("history_expired"), "Installed agent history expired")
        events.extend(page["events"])
        if not page["events"]:
            return events
        require(page["cursor"] > after, "Event cursor did not advance")
        after = page["cursor"]


def terminal_json(output):
    """A PTY merges human diagnostics with the response, which starts a line."""
    lines = output.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if line.startswith(b'{'):
            return json.loads(b''.join(lines[index:]))
    raise ValueError('Terminal output has no JSON response')


def mock_login(command, *, env=None):
    """Use the real registration path with an explicitly selected fake provider."""
    master, slave = pty.openpty()
    process = subprocess.Popen(
        command + ["auth", "login", "mock"], stdin=slave, stdout=slave, stderr=slave, env=env
    )
    os.close(slave)
    output = bytearray()
    deadline = time.monotonic() + 30
    def cleanup():
        def stop():
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
                    raise
        cleanup_all(lambda: os.close(master), stop)

    with finalizer(cleanup):
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.1)
            if ready:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not data:
                    break
                output.extend(data)
                require(len(output) < 1024 * 1024, "Mock login output exceeded limit")
            elif process.poll() is not None:
                break
        require(process.wait(timeout=5) == 0, output.decode(errors="replace"))
        require(terminal_json(output)["state"] == "authenticated", "Mock registration failed")


def verified_bootstrap(assets, work):
    """Execute a private snapshot whose consumed bytes match the admission record."""
    import re
    from .files import atomic_copy
    matches = [line.split() for line in (assets / 'SHA256SUMS').read_text().splitlines()
               if len(line.split()) == 2 and line.split()[1] == 'install.sh']
    require(len(matches) == 1 and re.fullmatch('[0-9a-f]{64}', matches[0][0]),
            'Missing or ambiguous install.sh checksum')
    source, snapshot = assets / 'install.sh', work / 'install.sh'
    atomic_copy(source, snapshot, expected={'sha256': matches[0][0], 'bytes': source.stat().st_size},
                mode=0o700, create_once=True)
    return snapshot
