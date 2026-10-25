"""Exercise the real CLI through a controlling terminal using a fake provider."""

import errno
import os
import pty
import select
import signal
import sys
import time

pid, fd = pty.fork()
if pid == 0:
    if os.environ.get("HIVE_PTY_BLOCK_HUP"):
        signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGHUP})
    os.execv(sys.argv[1], sys.argv[1:])
data = bytearray()
sent = False
deadline = time.monotonic() + 20
try:
    while time.monotonic() < deadline:
        if select.select([fd], [], [], 0.1)[0]:
            try:
                chunk = os.read(fd, 65536)
            except OSError as exc:
                if exc.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            data.extend(chunk)
            hangup = os.environ.get("HIVE_PTY_HANGUP")
            if (hangup == "ready" and b"fake-login: enter confirmation" in data) or (
                hangup == "complete" and b"fake-login: saved" in data
            ):
                os.close(fd)
                fd = None
                deadline = min(deadline, time.monotonic() + 8)
                break
            if (
                b"raw-ready" in data
                if os.environ.get("HIVE_PTY_RAW")
                else b"fake-login: enter confirmation" in data
            ) and not sent:
                action = os.environ.get("HIVE_PTY_CANCEL")
                os.write(
                    fd,
                    (
                        b"\x03"
                        if action
                        else (b"confirm\r" if os.environ.get("HIVE_PTY_RAW") else b"confirm\n")
                    ),
                )
                if action == "double":
                    time.sleep(0.05)
                    os.write(fd, b"\x03")
                sent = True
    else:
        print("login timeout", file=sys.stderr)
        sys.exit(124)
    while time.monotonic() < deadline:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            break
        time.sleep(0.01)
    else:
        print("login did not stop after terminal closure", file=sys.stderr)
        sys.exit(124)
    pid = None
    sys.stdout.buffer.write(data)
    sys.exit(os.waitstatus_to_exitcode(status))
finally:
    if fd is not None:
        os.close(fd)
    if pid is not None:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
