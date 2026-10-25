"""Drive a full-screen command through a terminal and check what its screen shows.

Usage: pty_attach.py COMMAND [ARGS...] with the steps as JSON in HIVE_PTY_STEPS.
Each step is ["wait", TEXT] (until the screen shows TEXT), ["gone", TEXT] (until
it no longer does), ["absent", TEXT], ["send", KEYS], ["sleep", SECONDS] or
["hangup", SECONDS]: close the terminal, as a closed window does, and require
the command to exit on its own within SECONDS. ["revoke", SECONDS] does the same
by revoking the terminal while keeping it open, as macOS does when a window
closes. ["click", TEXT] presses and
releases the left button on the first cell showing TEXT, and ["wheel", TEXT, N]
sends N wheel steps there (up when N > 0, down when N < 0), both as SGR mouse
reports. The script prints one JSON
result: whether every step passed, the failing step, the command's exit status
and the screen.
"""

import codecs
import ctypes
import ctypes.util
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time

ROWS, COLUMNS = 30, 100
STEP_TIMEOUT = 20
EXIT_TIMEOUT = 10
SEQUENCE = re.compile(
    r"\x1b\[([?>]?)([0-9;]*)([@-~])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[78=>]"
)


class Screen:
    """Enough of a VT100 for the view: cursor moves, clears and printed text."""

    def __init__(self):
        self.cells = [[" "] * COLUMNS for _ in range(ROWS)]
        self.row = self.column = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")

    def feed(self, data):
        text = self.pending + self.decoder.decode(data)
        self.pending = ""
        position = 0
        while position < len(text):
            if text[position] == "\x1b":
                match = SEQUENCE.match(text, position)
                if match is None:
                    # An escape split across reads finishes with the next chunk.
                    if len(text) - position < 64:
                        self.pending = text[position:]
                        return
                    position += 1
                    continue
                if match.group(3):
                    self.control(match.group(1), match.group(2), match.group(3))
                position = match.end()
                continue
            self.character(text[position])
            position += 1

    def control(self, private, parameters, final):
        values = [int(value) if value else 0 for value in parameters.split(";")]
        first = values[0] or 1
        if private:
            return
        if final in "Hf":
            row = values[0] if values[0] else 1
            column = values[1] if len(values) > 1 and values[1] else 1
            self.row, self.column = min(row, ROWS) - 1, min(column, COLUMNS) - 1
        elif final == "J":
            start = (self.row, self.column) if values[0] == 0 else (0, 0)
            for row in range(start[0], ROWS):
                for column in range(start[1] if row == start[0] else 0, COLUMNS):
                    self.cells[row][column] = " "
        elif final == "K":
            start = self.column if values[0] == 0 else 0
            for column in range(start, COLUMNS):
                self.cells[self.row][column] = " "
        elif final == "A":
            self.row = max(0, self.row - first)
        elif final == "B":
            self.row = min(ROWS - 1, self.row + first)
        elif final == "C":
            self.column = min(COLUMNS - 1, self.column + first)
        elif final == "D":
            self.column = max(0, self.column - first)
        elif final == "G":
            self.column = min(first, COLUMNS) - 1

    def character(self, character):
        if character == "\r":
            self.column = 0
        elif character == "\n":
            if self.row == ROWS - 1:
                self.cells.pop(0)
                self.cells.append([" "] * COLUMNS)
            else:
                self.row += 1
        elif character == "\b":
            self.column = max(0, self.column - 1)
        elif character >= " ":
            self.cells[self.row][self.column] = character
            self.column = min(COLUMNS - 1, self.column + 1)

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells)

    def find(self, value):
        """The 1-based column and row of the first cell showing value."""
        for row, cells in enumerate(self.cells):
            column = "".join(cells).find(value)
            if column >= 0:
                return column + 1, row + 1
        return None


def read_into(fd, screen, seconds):
    """Read output for up to `seconds`; False once the terminal has closed."""
    if not select.select([fd], [], [], seconds)[0]:
        return True
    try:
        chunk = os.read(fd, 65536)
    except OSError:
        return False
    if not chunk:
        return False
    screen.feed(chunk)
    return True


def run(fd, steps, screen):
    """The index of the failing step, or of a hangup step, or None."""
    for index, step in enumerate(steps):
        kind, value = step[0], step[1]
        if kind in ("hangup", "revoke"):
            return index
        if kind in ("click", "wheel"):
            deadline = time.monotonic() + STEP_TIMEOUT
            while (at := screen.find(value)) is None:
                if time.monotonic() > deadline or not read_into(fd, screen, 0.1):
                    return index
            column, row = at
            if kind == "click":
                report = f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m"
            else:
                button = 64 if step[2] > 0 else 65
                report = f"\x1b[<{button};{column};{row}M" * abs(step[2])
            os.write(fd, report.encode())
            continue
        if kind == "send":
            os.write(fd, value.encode())
        elif kind == "sleep":
            deadline = time.monotonic() + value
            while time.monotonic() < deadline and read_into(fd, screen, 0.05):
                pass
        elif kind == "absent":
            read_into(fd, screen, 0.2)
            if value in screen.text():
                return index
        else:
            shown = kind == "wait"
            deadline = time.monotonic() + STEP_TIMEOUT
            while (value in screen.text()) != shown:
                if time.monotonic() > deadline or not read_into(fd, screen, 0.1):
                    return index
    return None


def wait(pid, seconds, fd, screen):
    """The exit status if the command exits within `seconds`, reading its
    output meanwhile while the terminal is open; otherwise None."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        done, raw = os.waitpid(pid, os.WNOHANG)
        if done:
            return os.waitstatus_to_exitcode(raw)
        if fd is None:
            time.sleep(0.05)
        else:
            read_into(fd, screen, 0.05)
    return None


def main():
    steps = json.loads(os.environ["HIVE_PTY_STEPS"])
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLUMNS, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.execv(sys.argv[1], sys.argv[1:])
    screen = Screen()
    status = None
    try:
        failed = run(fd, steps, screen)
        if failed is not None and steps[failed][0] == "hangup":
            # Close the terminal as a closed window does: the command must
            # exit on its own.
            os.close(fd)
            fd = None
            status = wait(pid, steps[failed][1], fd, screen)
            if status is not None:
                pid, failed = None, None
        elif failed is not None and steps[failed][0] == "revoke":
            libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
            libc.ptsname.restype = ctypes.c_char_p
            if libc.revoke(libc.ptsname(fd)) == 0:
                status = wait(pid, steps[failed][1], None, screen)
                if status is not None:
                    pid, failed = None, None
        elif failed is None:
            status = wait(pid, EXIT_TIMEOUT, fd, screen)
            if status is not None:
                pid = None
    finally:
        # Close the terminal first: an exiting child can wait for its unread
        # output to drain, which would deadlock with waitpid below.
        if fd is not None:
            os.close(fd)
        if pid is not None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
    print(json.dumps({"failed_step": failed, "status": status, "screen": screen.text()}))


main()
