"""Exercise the release helper's gated spawn without a provider or model service."""

import json
import os
import shlex
import signal
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path

from mock_common import publish_copy

def receive(channel):
    envelope = json.loads(channel.readline())
    assert set(envelope) == {"protocol", "request"}
    assert envelope["protocol"] == 1
    return envelope["request"]


root = Path.cwd()
for name in ("hive-bash", "hive-shell"):
    publish_copy(sys.argv[1], root / name, mode=0o500)
listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
listener.bind(str(root / "guard.sock"))
listener.listen()
listener.settimeout(10)
for mode in ("cancel-before-gate", "kill-before-spawn"):
    token = str(uuid.uuid4())
    marker = root / mode
    command = "# HIVE_SHELL_GUARD:" + token + "\ntouch " + shlex.quote(str(marker))
    env = dict(os.environ, HIVE_SHELL_GUARD_SOCKET=str(root / "guard.sock"))
    process = subprocess.Popen(
        [str(root / "hive-bash"), "-c", shlex.join([str(root / "hive-shell"), command])],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        stream, _ = listener.accept()
        stream.settimeout(10)
        with stream, stream.makefile("rwb", buffering=0) as channel:
            register = receive(channel)
            assert register["op"] == "register" and register["token"] == token
            if mode == "kill-before-spawn":
                os.kill(register["pid"], signal.SIGKILL)
                assert channel.readline() == b""
            else:
                channel.write(b"ready\n")
                spawned = receive(channel)
                assert spawned["op"] == "spawned"
                os.kill(register["pid"], signal.SIGTERM)
                # The helper is blocked awaiting our attach reply, so cancellation
                # must already be pending when it considers opening the gate.
                time.sleep(0.1)
                channel.write(b"attached\n")
                finished = receive(channel)
                assert finished["op"] == "finished"
                try:
                    os.killpg(spawned["pid"], 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError("child group survived gate cancellation")
                channel.write(b"clean\n")
        process.communicate(timeout=15)
        assert process.returncode != 0
        assert not marker.exists(), "cancelled command executed"
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)
