"""Complete record boundaries and explicit large-stream limits."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from . import files


class CompleteLineTests(unittest.TestCase):
    def test_prepared_tree_has_one_commit_point_and_keeps_foreign_target(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source, destination = root / 'staging', root / 'bundle'
            source.mkdir(mode=0o700)
            files.atomic_write_bytes(source / 'weights', b'complete')
            destination.mkdir(mode=0o700)
            files.atomic_write_bytes(destination / 'foreign', b'preserved')
            with self.assertRaises(files.PublicationError) as error:
                files.publish_prepared(source, destination)
            self.assertFalse(error.exception.published)
            self.assertEqual((destination / 'foreign').read_bytes(), b'preserved')
            self.assertTrue(source.exists())
            published = root / 'published'
            sync = files.os.fsync
            def fail_after_commit(fd):
                if published.exists():
                    raise OSError('injected post-publication sync')
                return sync(fd)
            with patch.object(files.os, 'fsync', side_effect=fail_after_commit):
                with self.assertRaises(files.PublicationError) as error:
                    files.publish_prepared(source, published)
            self.assertTrue(error.exception.published)
            self.assertEqual(error.exception.retained, str(published))
            self.assertFalse(source.exists())
            self.assertEqual((published / 'weights').read_bytes(), b'complete')

    def test_stream_short_write_poison_cannot_splice_a_later_record(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'trace'
            write = files.os.write
            with files.AppendStream(path, create_once=True, durable=False) as stream:
                with patch.object(files.os, 'write', side_effect=lambda fd, b: write(fd, b[:3])):
                    with self.assertRaisesRegex(OSError, 'Short append'):
                        stream.append_jsonl({'first': 1})
                before = path.read_bytes()
                with self.assertRaisesRegex(ValueError, 'requires tail recovery'):
                    stream.append_jsonl({'second': 2})
                self.assertEqual(path.read_bytes(), before)
            with self.assertRaisesRegex(ValueError, 'Incomplete authoritative'):
                with files.AppendStream(path):
                    pass

    def test_raw_process_logs_preserve_arbitrary_tails_on_reopen(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'stderr'
            with files.raw_log(path) as log:
                log.write(b'partial\xff')
            with files.raw_log(path) as log:
                log.write(b'next\n')
            self.assertEqual(path.read_bytes(), b'partial\xffnext\n')
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_partial_utf8_tail_is_reported_without_decoding_and_repolled(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'events'
            prefix = b'{"ok":1}\n'
            files.atomic_write_bytes(path, prefix + b'{"next":"\xe2')
            with files.CompleteLines(path, tail_policy='report') as reader:
                self.assertEqual(list(reader), [prefix.decode()])
                self.assertEqual(reader.incomplete_tail, b'{"next":"\xe2')
                offset = reader.offset
            self.assertEqual(offset, len(prefix))
            with files.CompleteLines(path) as reader, self.assertRaises(files.IncompleteTail) as error:
                list(reader)
            self.assertEqual(error.exception.offset, offset)
            files.atomic_write_bytes(path, prefix + '{"next":"€"}\n'.encode())
            with files.CompleteLines(path, offset=offset) as reader:
                self.assertEqual([json.loads(line) for line in reader], [{'next': '€'}])
                self.assertEqual(reader.offset, path.stat().st_size)

    def test_snapshot_does_not_admit_later_bytes_and_offsets_are_boundaries(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'events'
            files.atomic_write_bytes(path, b'1\n')
            with files.CompleteLines(path) as reader:
                files.append_record(path, b'2\n', durable=False)
                self.assertEqual(list(reader), ['1\n'])
                self.assertEqual(reader.offset, 2)
            for offset in (1, 5, -1, True):
                with self.subTest(offset=offset), self.assertRaises(ValueError):
                    with files.CompleteLines(path, offset=offset):
                        pass

    def test_complete_malformed_lines_never_become_partial_tail(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'events'
            for data, kind in ((b'{broken}\n', json.JSONDecodeError), (b'"\xff"\n', UnicodeDecodeError)):
                files.atomic_write_bytes(path, data)
                with files.CompleteLines(path, tail_policy='report') as reader:
                    with self.assertRaises(kind):
                        [json.loads(line) for line in reader]
                    self.assertEqual(reader.incomplete_tail, b'')

    def test_large_stream_opt_in_preserves_default_and_rejects_overflow(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'audit'
            record = {'reply': 'x' * (1024 * 1024)}
            with self.assertRaisesRegex(ValueError, 'exceeds'):
                files.append_jsonl_record(path, record)
            self.assertFalse(path.exists())
            limit = 2 * 1024 * 1024
            files.append_jsonl_record(path, record, max_record_bytes=limit, durable=False)
            with files.CompleteLines(path) as reader, self.assertRaisesRegex(ValueError, 'exceeds'):
                list(reader)
            with files.CompleteLines(path, max_record_bytes=limit) as reader:
                self.assertEqual([json.loads(line) for line in reader], [record])
            before = path.read_bytes()
            with self.assertRaisesRegex(ValueError, 'exceeds'):
                files.append_jsonl_record(path, {'reply': 'x' * limit}, max_record_bytes=limit)
            self.assertEqual(path.read_bytes(), before)
            for limit in (0, True, files.MAX_RECORD_LIMIT + 1):
                with self.subTest(limit=limit), self.assertRaises(ValueError):
                    files.append_record(path, b'1\n', max_record_bytes=limit)

    def test_raw_append_rejects_multiple_or_incomplete_records_before_open(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'trace'
            for record in (b'', b'torn', b'one\ntwo\n', b'\0\n', 'text\n'):
                with self.subTest(record=record), self.assertRaises(ValueError):
                    files.append_record(path, record)
                self.assertFalse(path.exists())
