"""Give setup a controlling tty while its stdin remains an unread installer pipe."""
import errno
import json
import os
import pty
import select
import signal
import subprocess
import sys
import time

sentinel = b"INSTALLER_PIPE_MUST_REMAIN_UNREAD\n"
pid, fd = pty.fork()
if pid == 0:
    read_fd, write_fd = os.pipe()
    os.write(write_fd, sentinel)
    os.close(write_fd)
    code = subprocess.call(sys.argv[1:], stdin=read_fd)
    remaining = os.read(read_fd, len(sentinel))
    os.close(read_fd)
    assert remaining == sentinel, repr(remaining)
    print("SETUP_PIPE_PRESERVED", flush=True)
    sys.exit(code)
answers = [(needle.encode(), answer.encode() + b"\n") for needle, answer in json.loads(os.environ["HIVE_SETUP_ANSWERS"])]
data = bytearray()
deadline = time.monotonic() + 60
try:
    while time.monotonic() < deadline:
        if select.select([fd], [], [], .1)[0]:
            try:
                chunk = os.read(fd, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            data.extend(chunk)
            if answers and answers[0][0] in data:
                _, answer = answers.pop(0)
                os.write(fd, answer)
    else:
        raise TimeoutError(data.decode(errors="replace"))
    _, status = os.waitpid(pid, 0)
    pid = None
    sys.stdout.buffer.write(data)
    assert not answers, answers
    assert b"SETUP_PIPE_PRESERVED" in data
    sys.exit(os.waitstatus_to_exitcode(status))
finally:
    os.close(fd)
    if pid is not None:
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
