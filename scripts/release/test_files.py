"""Fault and concurrency contracts for the product publication helpers."""
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from concurrent.futures import ThreadPoolExecutor
from . import files


class AtomicFiles(unittest.TestCase):
    def test_replace_faults_report_the_publication_boundary(self):
        for phase in ("before", "after"):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as temp:
                path = Path(temp) / "state"
                files.atomic_write_bytes(path, b"before")
                fault = patch.object(files.os, "fsync", side_effect=OSError("injected")) if phase == "before" else patch.object(files, "_sync_parent", side_effect=OSError("injected"))
                with fault, self.assertRaises(files.PublicationError) as caught:
                    files.atomic_write_bytes(path, b"after")
                self.assertEqual(caught.exception.published, phase == "after")
                self.assertEqual(path.read_bytes(), b"after" if phase == "after" else b"before")
                self.assertEqual(list(path.parent.iterdir()), [path])

    def test_only_complete_replacements_and_one_create_once_winner(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "state"
            versions = (b"a" * 65536, b"b" * 65536)
            files.atomic_write_bytes(path, versions[0], durable=False)
            stop = threading.Event()
            observations = []
            def read():
                while not stop.is_set():
                    observations.append(path.read_bytes() in versions)
            with ThreadPoolExecutor(max_workers=1) as pool:
                reader = pool.submit(read)
                try:
                    for i in range(40):
                        files.atomic_write_bytes(path, versions[i % 2], durable=False)
                finally:
                    stop.set()
                reader.result()
            self.assertTrue(observations)
            self.assertTrue(all(observations))
            barrier = threading.Barrier(8)
            once = Path(temp) / "once"
            def create(n):
                barrier.wait()
                try:
                    files.atomic_write_bytes(once, bytes([n]) * 100, create_once=True, durable=False)
                    return True
                except files.PublicationError as error:
                    self.assertIsInstance(error.__cause__, FileExistsError)
                    self.assertFalse(error.published)
                    return False
            with ThreadPoolExecutor(max_workers=8) as pool:
                self.assertEqual(sum(pool.map(create, range(8))), 1)
            content = once.read_bytes()
            self.assertEqual(content, content[:1] * 100)
            self.assertEqual(once.stat().st_mode & 0o777, 0o600)
            link = Path(temp) / "link"
            link.symlink_to(once)
            with self.assertRaises(ValueError):
                files.atomic_write_bytes(link, b"unsafe")
            self.assertEqual(once.read_bytes(), content)

    def test_short_append_refuses_reopen_then_diagnostics_preserve_it(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "events"
            real_write = os.write
            with patch.object(files.os, "write", side_effect=lambda fd, data: real_write(fd, data[:3])):
                with self.assertRaisesRegex(OSError, "Short append"):
                    files.append_jsonl_record(path, dict(value=True))
            original = path.read_bytes()
            with self.assertRaisesRegex(ValueError, "inspect and restore"):
                files.append_jsonl_record(path, dict(next=True))
            files.append_jsonl_record(path, dict(next=True), tail_policy="rotate_diagnostic")
            self.assertEqual(path.with_name(".events.1.torn").read_bytes(), original)
            lines = files.read_jsonl_complete(path)
            self.assertEqual(lines["incomplete_tail"], b"")
            self.assertEqual(lines["records"][0]["event"], "log.recovered_tail")
            self.assertEqual(lines["records"][1], dict(next=True))

    def test_nul_tail_recovery_and_rotation_crash_keep_original_bytes(self):
        for moved in (False, True):
            with self.subTest(moved=moved), tempfile.TemporaryDirectory() as temp:
                path = Path(temp) / "events"
                data = b'{"a":1}\n\0\0'
                files.atomic_write_bytes(path, data)
                files.atomic_write_json(path.with_name(".events.recovery"), dict(segment=".events.1.torn", offset=8))
                if moved:
                    path.rename(path.with_name(".events.1.torn"))
                files.append_jsonl_record(path, dict(next=True), tail_policy="rotate_diagnostic", max_segments=1)
                self.assertEqual(path.with_name(".events.1.torn").read_bytes(), data)
                self.assertFalse(path.with_name(".events.recovery").exists())
                self.assertEqual(len(files.read_jsonl_complete(path)["records"]), 2)
                # A later damaged generation counts against the same budget.
                files.atomic_write_bytes(path, b'\0')
                files.append_jsonl_record(path, dict(next=True), tail_policy="rotate_diagnostic", max_segments=1)
                self.assertFalse(path.with_name(".events.1.torn").exists())
                self.assertEqual(path.with_name(".events.2.torn").read_bytes(), b'\0')

    def test_competing_appenders_never_splice_records(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "events"
            def append(n):
                for i in range(30):
                    files.append_jsonl_record(path, dict(writer=n, record=i))
            with ThreadPoolExecutor(max_workers=8) as pool:
                list(pool.map(append, range(8)))
            result = files.read_jsonl_complete(path)
            self.assertEqual(len(result["records"]), 240)
            self.assertEqual(result["incomplete_tail"], b"")
            self.assertEqual(len({(r["writer"], r["record"]) for r in result["records"]}), 240)

    def test_atomic_visible_never_syncs(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(files.os, "fsync", side_effect=AssertionError("unexpected sync")):
            files.atomic_write_bytes(Path(temp) / "derived", b"value", durable=False)

    def test_cleanup_failure_preserves_primary_and_has_no_false_retained_path(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "state"
            real_close = files.os.close
            def close(fd):
                real_close(fd)
                raise OSError("close failed")
            with patch.object(files.os, "fsync", side_effect=OSError("primary sync")), patch.object(files.os, "close", side_effect=close):
                with self.assertRaises(files.PublicationError) as caught:
                    files.atomic_write_bytes(path, b"value")
            self.assertIn("primary sync", str(caught.exception.__cause__))
            self.assertTrue(caught.exception.cleanup)
            self.assertIsNone(caught.exception.retained)

    def test_create_once_has_one_link_after_publication_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "journal"
            with patch.object(files, "_sync_parent", side_effect=OSError("crash boundary")):
                with self.assertRaises(files.PublicationError) as caught:
                    files.atomic_write_bytes(path, b"whole", create_once=True)
            self.assertTrue(caught.exception.published)
            self.assertEqual(path.stat().st_nlink, 1)
            self.assertEqual(files._read_private(path, 100), b"whole")
            self.assertEqual(list(path.parent.iterdir()), [path])

    def test_lock_is_released_despite_duplicate_and_reader_uses_only_lf(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "events"
            real_flock = files.fcntl.flock
            duplicates = []
            def flock(fd, mode):
                result = real_flock(fd, mode)
                if mode == files.fcntl.LOCK_EX:
                    duplicates.append(os.dup(fd))
                return result
            try:
                with patch.object(files.fcntl, "flock", side_effect=flock):
                    with files.record_lock(path):
                        pass
                with files._owned_fd(files._private_fd(path.with_name(".events.lock"), os.O_RDWR)) as fd:
                    real_flock(fd, files.fcntl.LOCK_EX | files.fcntl.LOCK_NB)
                    real_flock(fd, files.fcntl.LOCK_UN)
            finally:
                for fd in duplicates:
                    os.close(fd)
            files.atomic_write_bytes(path, b'{"a":1}\r{"b":2}\n')
            with self.assertRaises(json.JSONDecodeError):
                files.read_jsonl_complete(path)

    def test_reopen_finishes_retention_without_a_pending_journal(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "events"
            for name in (".events.1.torn", ".events.2.log", ".events.3.torn"):
                files.atomic_write_bytes(path.with_name(name), b"preserved")
            files.append_jsonl_record(path, dict(ok=True), tail_policy="rotate_diagnostic", max_segments=1)
            self.assertEqual([p.name for _, p in files._segments(path)], [".events.3.torn"])

    def test_torn_source_is_flushed_before_recovery_intent(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "events"
            files.atomic_write_bytes(path, b"torn", durable=False)
            events = []
            real_sync, real_intent = files._sync_source, files.atomic_write_json
            def sync(source):
                real_sync(source)
                events.append("source")
            def intent(*args, **kwargs):
                self.assertEqual(events, ["source"])
                events.append("intent")
                return real_intent(*args, **kwargs)
            with patch.object(files, "_sync_source", side_effect=OSError("source sync failed")):
                with self.assertRaisesRegex(OSError, "source sync failed"):
                    files.append_jsonl_record(path, dict(ok=True), tail_policy="rotate_diagnostic")
            self.assertFalse(path.with_name(".events.recovery").exists())
            self.assertEqual(path.read_bytes(), b"torn")
            with patch.object(files, "_sync_source", side_effect=sync), patch.object(files, "atomic_write_json", side_effect=intent):
                files.append_jsonl_record(path, dict(ok=True), tail_policy="rotate_diagnostic")
            self.assertEqual(events, ["source", "intent"])


if __name__ == "__main__":
    unittest.main()
