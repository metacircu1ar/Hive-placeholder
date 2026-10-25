"""Atomic publication and coordinated complete-record append mechanics."""
from contextlib import ExitStack, contextmanager
import ctypes
import errno
import fcntl
import json
import hashlib
import secrets
import os
from pathlib import Path
import stat
import sys


class PublicationError(OSError):
    def __init__(self, cause, *, published, cleanup=None, retained=None):
        super().__init__(getattr(cause, "errno", None), str(cause))
        self.published = published
        self.cleanup = cleanup
        self.retained = retained


@contextmanager
def _owned_fd(fd):
    """Close once; a cleanup failure must not replace the operation's cause."""
    try:
        yield fd
    except BaseException as cause:
        try:
            os.close(fd)
        except OSError as cleanup:
            cause.cleanup_errors = getattr(cause, "cleanup_errors", ()) + (cleanup,)
        raise
    else:
        os.close(fd)


def _sync_parent(path, *, fd=None):
    if fd is not None:
        os.fsync(fd)
        return
    with _owned_fd(os.open(Path(path).parent, os.O_RDONLY | os.O_DIRECTORY)) as parent:
        os.fsync(parent)


def sync_directory(path):
    with _owned_fd(os.open(path, os.O_RDONLY | os.O_DIRECTORY)) as fd:
        os.fsync(fd)


def create_directory(path, *, exist_ok=False):
    """Repeat ancestry barriers even when recovering previously created entries."""
    path = Path(path).absolute()
    path.mkdir(parents=True, exist_ok=exist_ok)
    for parent in (path, *path.parents):
        try:
            sync_directory(parent)
        except OSError as error:
            # A read-only filesystem cannot contain entries newly made here.
            if error.errno != errno.EROFS:
                raise


def _regular_at(name, directory):
    try:
        metadata = os.stat(name, dir_fd=directory, follow_symlinks=False)
    except FileNotFoundError:
        return
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError("Publication target must be a regular file")


def _rename_no_replace(source, destination, *, source_fd=-100, destination_fd=-100):
    """Native one-step publication, optionally relative to held directories."""
    source, destination = os.fsencode(source), os.fsencode(destination)
    if b"\0" in source or b"\0" in destination:
        raise ValueError("Embedded NUL in publication path")
    libc = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "darwin":
        rename, flag = libc.renameatx_np, 4  # RENAME_EXCL
        # Darwin's AT_FDCWD differs from Linux's.
        source_fd = -2 if source_fd == -100 else source_fd
        destination_fd = -2 if destination_fd == -100 else destination_fd
    elif sys.platform.startswith("linux"):
        rename, flag = libc.renameat2, 1  # RENAME_NOREPLACE
    else:
        raise NotImplementedError("Atomic no-replace publication requires macOS or Linux")
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    if rename(source_fd, source, destination_fd, destination, flag) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))


def _identity(info):
    return info.st_dev, info.st_ino, stat.S_IFMT(info.st_mode), info.st_uid


class _Preparation:
    def __init__(self, path, parent):
        self.path, self.parent = path, parent
        self.directory = _identity(os.fstat(parent))
        self.fd = self.stream = self.temporary = self.identity = None
        self.published = False

    def prepare(self, mode):
        for _ in range(128):
            name = '.hive-tmp-' + secrets.token_hex(12)
            try:
                fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.parent)
            except FileExistsError:
                continue
            self.fd, self.temporary = fd, name
            self.identity = _identity(os.fstat(fd))
            os.fchmod(fd, mode)  # Final mode before the first byte, irrespective of umask.
            self.stream = os.fdopen(fd, 'wb', closefd=False)
            return
        raise FileExistsError('Cannot reserve a unique publication name')

    def check_parent(self):
        if _identity(self.path.parent.stat()) != self.directory:
            raise OSError('Publication parent changed; retaining preparation')

    def check_preparation(self):
        info = os.stat(self.temporary, dir_fd=self.parent, follow_symlinks=False)
        if _identity(info) != self.identity or not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise OSError('Publication preparation changed; retaining unexpected entry')

    def publish(self, durable, create_once):
        self.stream.flush()
        if durable:
            os.fsync(self.fd)
        self.stream.close()
        self.check_parent()
        self.check_preparation()
        _regular_at(self.path.name, self.parent)
        if create_once:
            _rename_no_replace(self.temporary, self.path.name, source_fd=self.parent, destination_fd=self.parent)
        else:
            os.replace(self.temporary, self.path.name, src_dir_fd=self.parent, dst_dir_fd=self.parent)
        self.published = True
        if durable:
            _sync_parent(self.path, fd=self.parent)
        self.check_parent()

    def cleanup(self):
        errors, retained = [], None
        if self.stream is not None and not self.stream.closed:
            try:
                self.stream.close()
            except OSError as error:
                errors.append(error)
        # A published name consumed our preparation. Never remove a subsequently
        # recreated entry there, including during KeyboardInterrupt/SystemExit.
        if self.temporary is not None and not self.published:
            try:
                self.check_parent()
                self.check_preparation()
                os.unlink(self.temporary, dir_fd=self.parent)
            except FileNotFoundError:
                pass
            except OSError as error:
                errors.append(error)
                retained = str(self.path.parent / self.temporary)
        # Keep even an unlinked preparation inode pinned until the namespace
        # decision is complete, so a foreign entry cannot reuse its identity.
        if self.fd is not None:
            fd, self.fd = self.fd, None
            try:
                os.close(fd)
            except OSError as error:
                errors.append(error)
        return errors, retained


def atomic_stream(path, write, *, mode=0o600, durable=True, create_once=False):
    """Stream into a private inode, then publish and sync through a held parent."""
    if not isinstance(mode, int) or mode < 0 or mode & ~0o777:
        raise ValueError('Invalid atomic file mode')
    path = Path(path)
    parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    preparation = None
    try:
        with _owned_fd(parent):
            _regular_at(path.name, parent)
            preparation = _Preparation(path, parent)
            try:
                preparation.prepare(mode)
                result = write(preparation.stream)
                preparation.publish(durable, create_once)
                fd, preparation.fd = preparation.fd, None
                os.close(fd)
                return result
            except BaseException as cause:
                errors, retained = preparation.cleanup()
                cleanup = tuple(getattr(cause, 'cleanup_errors', ())) + tuple(errors)
                if not isinstance(cause, Exception):
                    cause.published, cause.retained, cause.cleanup_errors = preparation.published, retained, cleanup
                    raise
                raise PublicationError(cause, published=preparation.published, cleanup=cleanup,
                                       retained=retained) from cause
    except BaseException as cause:
        # Parent close can fail after a successful publication; keep that outcome.
        if preparation is None or isinstance(cause, PublicationError):
            raise
        if not isinstance(cause, Exception):
            cause.published = preparation.published
            raise
        raise PublicationError(cause, published=preparation.published,
                               cleanup=getattr(cause, 'cleanup_errors', ())) from cause


def atomic_write_bytes(path, data, **options):
    if not isinstance(data, bytes):
        raise ValueError('Invalid atomic file bytes')
    atomic_stream(path, lambda output: output.write(data), **options)


def copy_stream(source, output, *, expected=None):
    """Hash exactly the bytes consumed, not a different preflight open."""
    value, size = hashlib.sha256(), 0
    with Path(source).open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            if expected is not None and size + len(chunk) > expected['bytes']:
                raise ValueError(f'Consumed frozen file identity mismatch: {source}')
            output.write(chunk)
            value.update(chunk)
            size += len(chunk)
    record = dict(sha256=value.hexdigest(), bytes=size)
    if expected is not None and any(record[key] != expected[key] for key in record):
        raise ValueError(f'Consumed frozen file identity mismatch: {source}')
    return record


def atomic_copy(source, destination, *, expected=None, **options):
    return atomic_stream(destination, lambda output: copy_stream(source, output, expected=expected), **options)


def _sync_prepared(fd):
    info = os.fstat(fd)
    if info.st_uid != os.geteuid():
        raise ValueError('Prepared entry has a foreign owner')
    if stat.S_ISDIR(info.st_mode):
        for name in os.listdir(fd):
            with _owned_fd(os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)) as child:
                _sync_prepared(child)
    elif not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ValueError('Prepared tree must contain only owned directories and single-link files')
    os.fsync(fd)


def publish_prepared(source, destination):
    """Durably publish an already complete owned file/tree without replacement.

    The caller exclusively owns the staging namespace and sets permissions before
    content. No cleanup runs here: uncertain publication retains caller evidence.
    """
    source, destination = Path(source), Path(destination)
    published = False
    try:
        with _owned_fd(os.open(source.parent, os.O_RDONLY | os.O_DIRECTORY)) as origin:
            with _owned_fd(os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)) as target:
                with _owned_fd(os.open(source.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=origin)) as entry:
                    identity = _identity(os.fstat(entry))
                    _sync_prepared(entry)
                    os.fsync(origin)
                    if (_identity(source.parent.stat()) != _identity(os.fstat(origin))
                            or _identity(destination.parent.stat()) != _identity(os.fstat(target))
                            or _identity(os.stat(source.name, dir_fd=origin, follow_symlinks=False)) != identity):
                        raise OSError('Prepared publication route changed')
                    _rename_no_replace(source.name, destination.name, source_fd=origin, destination_fd=target)
                    published = True
                    os.fsync(target)
                    os.fsync(origin)
                    if (_identity(destination.parent.stat()) != _identity(os.fstat(target))
                            or _identity(os.stat(destination.name, dir_fd=target, follow_symlinks=False)) != identity):
                        raise OSError('Published entry route changed')
    except BaseException as cause:
        if not isinstance(cause, Exception):
            cause.published, cause.retained = published, str(destination if published else source)
            raise
        raise PublicationError(cause, published=published, retained=str(destination if published else source),
                               cleanup=getattr(cause, 'cleanup_errors', ())) from cause


@contextmanager
def directory_lock(path):
    """Stable empty lock; never unlink it while another packager may hold it."""
    with record_lock(Path(path) / 'publication'):
        yield


def atomic_write_text(path, text, **options):
    atomic_write_bytes(path, text.encode("utf-8"), **options)


def atomic_write_json(path, value, **options):
    # Serialize first, so an encoding error cannot leave an incomplete file.
    atomic_write_text(path, json.dumps(value, indent=2, sort_keys=True) + "\n", **options)


def _private_fd(path, flags):
    fd = os.open(path, flags | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_nlink != 1 or info.st_mode & 0o077):
            raise ValueError("Append entry must be an owned private regular file")
    except BaseException:
        with _owned_fd(fd):
            raise
    return fd


def raw_log(path):
    """Private O_APPEND descriptor for arbitrary inherited stdout/stderr bytes.

    This is not a record protocol: child writes may split lines. Never parse it as
    JSONL, truncate it on reopen, or apply diagnostic JSON recovery to its bytes.
    """
    fd = _private_fd(path, os.O_RDWR | os.O_CREAT | os.O_APPEND)
    try:
        return os.fdopen(fd, 'ab', buffering=0)
    except BaseException:
        with _owned_fd(fd):
            raise


@contextmanager
def record_lock(path):
    """Stable lock; yield its fd for explicitly owned subprocess writers to retain."""
    path = Path(path)
    fd = _private_fd(path.with_name(f".{path.name}.lock"), os.O_RDWR | os.O_CREAT)
    with _owned_fd(fd):
        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            yield fd
        except BaseException as cause:
            try:
                fcntl.flock(fd, fcntl.LOCK_UN)
            except OSError as cleanup:
                cause.cleanup_errors = getattr(cause, "cleanup_errors", ()) + (cleanup,)
            raise
        else:
            fcntl.flock(fd, fcntl.LOCK_UN)


def _read_private(path, limit):
    try:
        fd = _private_fd(path, os.O_RDONLY)
    except FileNotFoundError:
        return None
    with _owned_fd(fd):
        with os.fdopen(fd, "rb", closefd=False) as stream:
            data = stream.read(limit + 1)
        if len(data) > limit:
            raise ValueError("Append recovery file exceeds size limit")
        return data


def _sync_source(path):
    with _owned_fd(_private_fd(path, os.O_RDONLY)) as fd:
        os.fsync(fd)


def _tail(path):
    try:
        fd = _private_fd(path, os.O_RDONLY)
    except FileNotFoundError:
        return None
    with _owned_fd(fd):
        end = os.fstat(fd).st_size
        if not end or os.pread(fd, 1, end - 1) == b"\n":
            return None
        while end:
            start = max(0, end - 8192)
            data = os.pread(fd, end - start, start)
            if len(data) != end - start:
                raise OSError("Append file changed during tail inspection")
            position = data.rfind(b"\n")
            if position >= 0:
                return start + position + 1
            end = start
        return 0


def _segments(path):
    prefix = f".{path.name}."
    found = []
    for item in path.parent.iterdir():
        name = item.name
        if not name.startswith(prefix) or item.suffix not in (".torn", ".log"):
            continue
        number = name[len(prefix):-len(item.suffix)]
        if not number.isascii() or not number.isdigit():
            continue
        fd = _private_fd(item, os.O_RDONLY)
        os.close(fd)
        found.append((int(number), item))
    return sorted(found)


def _next_segment(path):
    segments = _segments(path)
    number = segments[-1][0] + 1 if segments else 1
    return path.with_name(f".{path.name}.{number}.torn")


def _move_segment(path, destination):
    # This lock owns both names. Fail instead of replacing an unexpected segment.
    _rename_no_replace(path, destination)
    _sync_parent(path)


def _finish_recovery(path, journal, policy, max_segments):
    data = _read_private(journal, 4096)
    if data is None:
        return
    if policy != "rotate_diagnostic":
        raise ValueError("Diagnostic recovery pending; reopen with diagnostic rotation")
    intent = json.loads(data)
    if not isinstance(intent, dict) or set(intent) != {"segment", "offset"}:
        raise ValueError("Invalid append recovery record")
    name, offset = intent["segment"], intent["offset"]
    prefix = f".{path.name}."
    if (not isinstance(name, str) or not name.startswith(prefix) or not name.endswith(".torn")
            or not name[len(prefix):-5].isascii() or not name[len(prefix):-5].isdigit()
            or type(offset) is not int or offset < 0):
        raise ValueError("Invalid append recovery fields")
    segment = path.with_name(name)
    try:
        fd = _private_fd(segment, os.O_RDONLY)
    except FileNotFoundError:
        if _tail(path) != offset:
            raise ValueError("Recovery source changed; inspect append files")
        _move_segment(path, segment)
    else:
        os.close(fd)
    record = json.dumps(dict(event="log.recovered_tail", segment=name,
                             last_complete_offset=offset), sort_keys=True).encode() + b"\n"
    current = _read_private(path, 1024 * 1024)
    if current not in (None, b"", record):
        if _tail(path) is None:
            raise ValueError("Active stream changed during recovery")
        _move_segment(path, _next_segment(path))
    if current != record:
        atomic_write_bytes(path, record)
    journal.unlink()
    _sync_parent(path)
    _prune(path, max_segments)


def _prune(path, max_segments):
    segments = _segments(path)
    for _, old in segments[:max(0, len(segments) - max_segments)]:
        old.unlink()
    if len(segments) > max_segments:
        _sync_parent(path)


DEFAULT_RECORD_LIMIT = 1024 * 1024
MAX_RECORD_LIMIT = 128 * 1024 * 1024


def _record_limit(limit):
    if type(limit) is not int or not 1 <= limit <= MAX_RECORD_LIMIT:
        raise ValueError("Record limit must be between 1 byte and 128 MiB")
    return limit


def append_jsonl_record(path, value, **options):
    record = json.dumps(value, separators=(",", ":")).encode() + b"\n"
    append_record(path, record, **options)


def _validate_append(record, limit):
    if (not isinstance(record, bytes) or not record.endswith(b"\n")
            or b"\n" in record[:-1] or b"\0" in record):
        raise ValueError("Append record must be one complete line without NUL")
    if len(record) > limit:
        raise ValueError(f"Append record exceeds {limit} bytes")


def _prepare_append(path, policy, max_segments):
    journal = path.with_name(f".{path.name}.recovery")
    _finish_recovery(path, journal, policy, max_segments)
    offset = _tail(path)
    if offset is not None:
        if policy == "refuse":
            raise ValueError(f"Incomplete authoritative append tail after byte {offset}; inspect and restore the authoritative append file before retrying")
        segment = _next_segment(path)
        _sync_source(path)
        atomic_write_json(journal, dict(segment=segment.name, offset=offset), create_once=True)
        _finish_recovery(path, journal, policy, max_segments)
    if policy == "rotate_diagnostic":
        _prune(path, max_segments)


class AppendStream:
    """Own a logical stream while appending bounded records with one syscall.

    A failed write poisons the handle. Close/reopen explicitly to inspect a torn
    tail; never silently retry an uncertain record. Long-lived diagnostic streams
    can use durable=False to avoid introducing fsync into sampling hot paths.
    """
    def __init__(self, path, *, create_once=False, tail_policy="refuse", max_segments=4,
                 durable=True, max_record_bytes=DEFAULT_RECORD_LIMIT):
        if tail_policy not in ("refuse", "rotate_diagnostic") or type(max_segments) is not int or max_segments < 1:
            raise ValueError("Invalid append tail policy or retention")
        self.limit = _record_limit(max_record_bytes)
        self.path, self.policy, self.max_segments = Path(path), tail_policy, max_segments
        self.create_once, self.durable = create_once, durable
        self.fd, self.stack, self.poisoned = None, None, False

    def __enter__(self):
        if self.stack is not None:
            raise ValueError("Append stream already opened")
        self.stack = ExitStack()
        try:
            self.stack.enter_context(record_lock(self.path))
            if self.create_once:
                atomic_write_bytes(self.path, b"", create_once=True, durable=self.durable)
            _prepare_append(self.path, self.policy, self.max_segments)
            self.fd = self.stack.enter_context(_owned_fd(
                _private_fd(self.path, os.O_RDWR | os.O_CREAT | os.O_APPEND)))
        except BaseException:
            self.stack.__exit__(*sys.exc_info())
            raise
        return self

    def __exit__(self, *args):
        self.fd = None
        return self.stack.__exit__(*args)

    def close(self):
        self.__exit__(None, None, None)

    def append_jsonl(self, value):
        self.append_record(json.dumps(value, separators=(",", ":")).encode() + b"\n")

    def append_record(self, record):
        _validate_append(record, self.limit)
        if self.fd is None or self.poisoned:
            raise ValueError("Append stream is closed or requires tail recovery")
        try:
            if os.write(self.fd, record) != len(record):
                raise OSError("Short append record; reopen with tail recovery")
            if self.durable:
                os.fsync(self.fd)
                _sync_parent(self.path)
        except BaseException:
            self.poisoned = True
            raise


def append_record(path, record, **options):
    """One O_APPEND write under a stable lock; never extend an incomplete tail."""
    _validate_append(record, _record_limit(options.get('max_record_bytes', DEFAULT_RECORD_LIMIT)))
    with AppendStream(path, **options) as stream:
        stream.append_record(record)


class IncompleteTail(ValueError):
    def __init__(self, path, offset, size):
        super().__init__(f"Incomplete evidence in {path}: {size} bytes after byte {offset}")
        self.path, self.offset, self.size = Path(path), offset, size


class CompleteLines:
    """Bounded snapshot reader; offsets only advance past complete LF records.

    Final evidence fails on a torn tail by default. A live inspector may request
    tail_policy='report', inspect incomplete_tail, and resume at offset next poll.
    Neither mode decodes a partial line (including a partial UTF-8 character).
    """
    def __init__(self, path, *, offset=0, tail_policy="error", max_record_bytes=DEFAULT_RECORD_LIMIT):
        if type(offset) is not int or offset < 0 or tail_policy not in ("error", "report"):
            raise ValueError("Invalid complete-line offset or tail policy")
        self.path, self.offset, self.tail_policy = Path(path), offset, tail_policy
        self.limit = _record_limit(max_record_bytes)
        self.incomplete_tail = b""
        self.stream = None

    def __enter__(self):
        self.stream = self.path.open("rb")
        try:
            self.end = os.fstat(self.stream.fileno()).st_size
            if self.offset > self.end:
                raise ValueError("Complete-line offset exceeds file length")
            if self.offset:
                self.stream.seek(self.offset - 1)
                if self.stream.read(1) != b"\n":
                    raise ValueError("Complete-line offset is not a record boundary")
            self.stream.seek(self.offset)
        except BaseException:
            self.stream.close()
            raise
        return self

    def __exit__(self, *args):
        return self.stream.__exit__(*args)

    def __iter__(self):
        return self

    def __next__(self):
        if self.incomplete_tail or self.offset == self.end:
            raise StopIteration
        line = self.stream.readline(min(self.limit + 1, self.end - self.offset))
        if not line:
            raise OSError("Evidence file shrank while being read")
        if len(line) > self.limit:
            raise ValueError(f"Evidence record exceeds {self.limit} bytes")
        if not line.endswith(b"\n"):
            self.incomplete_tail = line
            if self.tail_policy == "error":
                raise IncompleteTail(self.path, self.offset, len(line))
            raise StopIteration
        text = line.decode("utf-8")
        self.offset += len(line)
        return text


def read_jsonl_complete(path, *, max_record_bytes=DEFAULT_RECORD_LIMIT):
    """Return complete decoded records and the unparsed tail with its offset."""
    with CompleteLines(path, tail_policy="report", max_record_bytes=max_record_bytes) as stream:
        records = [json.loads(line) for line in stream]
        return dict(records=records, incomplete_tail=stream.incomplete_tail,
                    last_complete_offset=stream.offset)
