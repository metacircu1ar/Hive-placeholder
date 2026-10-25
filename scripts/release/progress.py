"""Bounded standard-logging diagnostics; messages and exception arguments stay private.

Only the daemon writer touches a duplicate of stderr, without a logging handler or
Python stream lock. Shutdown can abandon that writer without logging.shutdown()
waiting on its I/O. Structured loss counts are best effort; finish() always returns
the current counts and whether the worker actually drained.
"""
from contextlib import contextmanager, ExitStack
import json
import logging
import os
from pathlib import Path
import queue
import re
import subprocess
import threading
import time

ROOT = str(Path(__file__).resolve().parents[2]) + os.sep
LOGGER = logging.getLogger('hive.tools')
LOGGER.addHandler(logging.NullHandler())
LOGGER.propagate = False
EVENTS = {'phase_started', 'phase_finished', 'heartbeat', 'exception', 'subprocess_failed',
          'subprocess_invalid_json', 'observation_failed', 'cleanup_failed', 'diagnostic_loss'}
PHASES = {'build', 'licenses', 'package', 'install', 'verify', 'fetch', 'daemon', 'login',
          'shutdown', 'observe', 'wait', 'models', 'publish', 'scenario',
          'installed_core', 'installed_hosted', 'installed_wizard'}
NUMBERS = {'count', 'bytes', 'elapsed_ms', 'returncode', 'stderr_bytes', 'stdout_bytes'}
FLAGS = {'success', 'partial', 'confirmed'}
IDENTIFIER = re.compile(r'^[A-Za-z_][A-Za-z0-9_]{0,63}$')


def identifier(value):
    return value if isinstance(value, str) and IDENTIFIER.fullmatch(value) else 'unknown'


def exception_details(error):
    """Retain bounded types/locations/errno, never args, source lines or native text."""
    result, seen = [], set()
    while error is not None and id(error) not in seen and len(result) < 8:
        seen.add(id(error))
        frames, trace = [], error.__traceback__
        while trace is not None and len(frames) < 8:
            code = trace.tb_frame.f_code
            name = os.path.normpath(code.co_filename)
            file = name[len(ROOT):] if name.startswith(ROOT) else '<external>'
            frames.append({'file': file[:160], 'function': identifier(code.co_name), 'line': trace.tb_lineno})
            trace = trace.tb_next
        item = {'type': identifier(type(error).__name__), 'frames': frames}
        if isinstance(error, OSError) and isinstance(error.errno, int):
            item['errno'] = error.errno
        if isinstance(error, (subprocess.CalledProcessError, subprocess.TimeoutExpired)):
            for name in ('stdout', 'stderr'):
                data = getattr(error, name, None)
                if isinstance(data, str):
                    data = data.encode('utf-8', errors='replace')
                item[name + '_bytes'] = len(data) if isinstance(data, bytes) else 0
            if isinstance(error, subprocess.CalledProcessError):
                item['returncode'] = error.returncode
        result.append(item)
        error = error.__cause__ or (None if error.__suppress_context__ else error.__context__)
    return result


def encode(record):
    if record.name == 'hive.tools':
        event = getattr(record, 'hive_event', None)
        fields = getattr(record, 'hive_fields', {})
        if event not in EVENTS or not isinstance(fields, dict):
            raise ValueError('Invalid diagnostic event')
        value = {'event': 'tools.' + event}
        for key, field in fields.items():
            if key == 'phase' and field in PHASES:
                value[key] = field
            elif key in NUMBERS and type(field) in (int, float) and -2**63 <= field < 2**63:
                value[key] = field
            elif key in FLAGS and type(field) is bool:
                value[key] = field
            else:
                raise ValueError('Invalid diagnostic field')
    else:
        # Library log messages and arguments may contain URLs, credentials or bodies.
        if record.levelno < logging.WARNING:
            return None
        value = {'event': 'tools.library_record'}
    level = 'error' if record.levelno >= logging.ERROR else 'warn' if record.levelno >= logging.WARNING else 'info'
    value.update(level=level, monotonic_ns=time.monotonic_ns())
    if record.exc_info and isinstance(record.exc_info[1], BaseException):
        value['causes'] = exception_details(record.exc_info[1])
    data = (json.dumps(value, ensure_ascii=True, allow_nan=False, separators=(',', ':')) + '\n').encode()
    if len(data) > 8192:
        raise ValueError('Diagnostic record exceeds bound')
    return data


class _Handler(logging.Handler):
    def __init__(self, owner):
        super().__init__()
        self.owner = owner

    def emit(self, record):
        try:
            data = encode(record)
        except Exception:
            self.owner.increment('invalid')
            return
        if data is not None:
            self.owner.admit(data)


class Session:
    def __init__(self, *, fd=2, slots=1024):
        if not 1 <= slots <= 65536:
            raise ValueError('Invalid diagnostic queue size')
        self.queue = queue.Queue(maxsize=slots)
        self.lock = threading.Lock()
        self.counts = dict(admitted=0, written=0, dropped=0, invalid=0, write_failed=0)
        self.closing = False
        self.done = threading.Event()
        self.fd = os.dup(fd)
        self.handler = _Handler(self)
        self.worker = threading.Thread(target=self._write, name='hive-tool-log', daemon=True)
        try:
            self.worker.start()
        except BaseException:
            with finalizer(lambda: os.close(self.fd)):
                raise

    def increment(self, name):
        with self.lock:
            self.counts[name] += 1

    def admit(self, data):
        with self.lock:
            if self.closing:
                self.counts['dropped'] += 1
                return
            try:
                self.queue.put_nowait(data)
            except queue.Full:
                self.counts['dropped'] += 1
            else:
                self.counts['admitted'] += 1

    def _write_once(self, data):
        try:
            if os.write(self.fd, data) != len(data):
                self.increment('write_failed')
                return False
        except OSError:
            self.increment('write_failed')
            return False
        return True

    def _write(self):
        try:
            while True:
                try:
                    data = self.queue.get(timeout=0.05)
                except queue.Empty:
                    with self.lock:
                        if self.closing:
                            break
                    continue
                if self._write_once(data):
                    self.increment('written')
            with self.lock:
                counts = dict(self.counts)
            if any(counts[k] for k in ('dropped', 'invalid', 'write_failed')):
                self._write_once((json.dumps({'event': 'tools.diagnostic_loss', **counts}) + '\n').encode())
        finally:
            try:
                os.close(self.fd)
            except OSError:
                self.increment('write_failed')
            self.done.set()

    def finish(self, timeout=0.25):
        with self.lock:
            self.closing = True
        confirmed = self.done.wait(timeout)
        with self.lock:
            return dict(self.counts, confirmed=confirmed)

    def __enter__(self):
        root = logging.getLogger()
        self.previous, self.level = root.handlers[:], root.level
        self.previous_tool, self.tool_level = LOGGER.handlers[:], LOGGER.level
        root.handlers = [self.handler]
        root.setLevel(logging.INFO)
        LOGGER.handlers = [self.handler]
        LOGGER.setLevel(logging.INFO)
        return self

    def __exit__(self, kind, error, traceback):
        if error is not None:
            event('exception', error=error, success=False)
        root = logging.getLogger()
        root.handlers = self.previous
        root.setLevel(self.level)
        LOGGER.handlers = self.previous_tool
        LOGGER.setLevel(self.tool_level)
        self.finish()
        # No worker ever holds this handler's lock, including at interpreter shutdown.
        self.handler.close()


def event(name, *, error=None, **fields):
    LOGGER.log(logging.ERROR if error is not None else logging.INFO, '',
               extra={'hive_event': name, 'hive_fields': fields},
               exc_info=(type(error), error, error.__traceback__) if error is not None else None)


@contextmanager
def phase(name, *, interval=10.0):
    if name not in PHASES or interval <= 0:
        raise ValueError('Invalid progress phase')
    started, stop = time.monotonic(), threading.Event()
    event('phase_started', phase=name)

    def heartbeat():
        while not stop.wait(interval):
            event('heartbeat', phase=name, elapsed_ms=int((time.monotonic() - started) * 1000))

    worker = threading.Thread(target=heartbeat, name='hive-tool-progress', daemon=True)
    worker.start()
    success = False
    try:
        yield
        success = True
    except SystemExit as error:
        success = error.code is None or error.code == 0
        raise
    finally:
        stop.set()
        worker.join(timeout=0.25)
        event('phase_finished', phase=name, success=success,
              elapsed_ms=int((time.monotonic() - started) * 1000))


def entry(work, *, name):
    """CLI entry suppresses Python's unsanitized default traceback rendering."""
    try:
        session = Session()
    except (OSError, RuntimeError):
        # No safe writer exists (closed stderr/resource exhaustion). Fail before
        # work begins; a synchronous stderr fallback could hang or expose text.
        return 1
    with session:
        try:
            with phase(name):
                work()
            return 0
        except SystemExit as error:
            if error.code is None or type(error.code) is int:
                return error.code or 0
            event('exception', error=error, success=False)
            return 1
        except KeyboardInterrupt as error:
            event('exception', error=error, success=False)
            return 130
        except Exception as error:
            event('exception', error=error, success=False)
            return 1


@contextmanager
def finalizer(cleanup):
    """Report a cleanup failure without replacing a failure already in flight."""
    primary = None
    try:
        yield
    except BaseException as error:
        primary = error
        raise
    finally:
        try:
            cleanup()
        except BaseException as error:
            event('cleanup_failed', error=error, partial=True)
            if primary is None:
                raise


def cleanup_all(*actions):
    """Attempt every cleanup in order, retaining the first failure and reporting later ones."""
    with ExitStack() as stack:
        for action in reversed(actions):
            stack.enter_context(finalizer(action))
