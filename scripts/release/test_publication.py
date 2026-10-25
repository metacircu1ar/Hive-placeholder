"""Identity-bound preparation/cleanup and streamed file publication faults."""
import io
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch
from . import files


class StreamingPublication(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.parent = self.root / 'directory'; self.parent.mkdir()
        self.path = self.parent / 'output'

    def test_source_failure_preserves_previous_file_and_removes_only_preparation(self):
        files.atomic_write_bytes(self.path, b'old')
        def write(output):
            output.write(b'partial')
            raise OSError('source failed')
        with self.assertRaises(files.PublicationError) as caught:
            files.atomic_stream(self.path, write)
        self.assertFalse(caught.exception.published)
        self.assertIn('source failed', str(caught.exception.__cause__))
        self.assertEqual(self.path.read_bytes(), b'old')
        self.assertEqual(list(self.parent.iterdir()), [self.path])

    def test_replaced_preparation_and_original_are_preserved(self):
        names = []
        def write(output):
            output.write(b'private'); output.flush()
            name = next(self.parent.glob('.hive-tmp-*')); names.append(name)
            name.rename(self.parent / 'saved')
            name.write_bytes(b'foreign')
        with self.assertRaises(files.PublicationError) as caught:
            files.atomic_stream(self.path, write)
        self.assertFalse(caught.exception.published)
        self.assertIsNotNone(caught.exception.retained)
        self.assertEqual(names[0].read_bytes(), b'foreign')
        self.assertEqual((self.parent / 'saved').read_bytes(), b'private')
        self.assertFalse(self.path.exists())

    def test_moved_parent_never_removes_original_or_foreign_preparation(self):
        for interrupt in [False, True]:
            with self.subTest(interrupt=interrupt):
                parent = self.parent / str(interrupt); parent.mkdir()
                moved = self.root / str(interrupt)
                names = []
                def write(output):
                    output.write(b'private'); output.flush()
                    name = next(parent.glob('.hive-tmp-*')).name; names.append(name)
                    parent.rename(moved); parent.mkdir()
                    (parent / name).write_bytes(b'foreign')
                    if interrupt:
                        raise KeyboardInterrupt()
                expected = KeyboardInterrupt if interrupt else files.PublicationError
                with self.assertRaises(expected) as caught:
                    files.atomic_stream(parent / 'output', write)
                self.assertFalse(caught.exception.published)
                self.assertIsNotNone(caught.exception.retained)
                self.assertEqual((parent / names[0]).read_bytes(), b'foreign')
                self.assertEqual((moved / names[0]).read_bytes(), b'private')

    def test_recreated_temporary_after_publication_is_never_deleted(self):
        for fault in [OSError('directory sync'), KeyboardInterrupt()]:
            with self.subTest(fault=type(fault)):
                real_replace, names = os.replace, []
                def replace(source, destination, **options):
                    real_replace(source, destination, **options)
                    names.append(self.parent / source)
                    fd = os.open(source, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=options['src_dir_fd'])
                    with os.fdopen(fd, 'wb') as output:
                        output.write(b'foreign')
                with patch.object(files.os, 'replace', side_effect=replace), patch.object(files, '_sync_parent', side_effect=fault):
                    with self.assertRaises(files.PublicationError if isinstance(fault, Exception) else KeyboardInterrupt) as caught:
                        files.atomic_write_bytes(self.path, b'complete')
                self.assertTrue(caught.exception.published)
                self.assertEqual(self.path.read_bytes(), b'complete')
                self.assertEqual(names[0].read_bytes(), b'foreign')

    def test_modes_are_final_before_write_and_close_failure_retains_publication(self):
        for mode in [0o600, 0o644, 0o700, 0o755]:
            def write(output):
                self.assertEqual(os.fstat(output.fileno()).st_mode & 0o777, mode)
                output.write(b'complete')
            files.atomic_stream(self.path, write, mode=mode)
            self.assertEqual(self.path.stat().st_mode & 0o777, mode)
        real_close = os.close
        def close(fd):
            directory = stat.S_ISDIR(os.fstat(fd).st_mode)
            real_close(fd)
            if directory:
                raise OSError('directory close failed')
        with patch.object(files.os, 'close', side_effect=close), self.assertRaises(files.PublicationError) as caught:
            files.atomic_write_bytes(self.path, b'new')
        self.assertTrue(caught.exception.published)
        self.assertEqual(self.path.read_bytes(), b'new')

    def test_acceptance_uses_a_verified_private_bootstrap_snapshot(self):
        from .acceptance import verified_bootstrap
        import hashlib
        import subprocess
        source = self.parent / 'install.sh'
        body = b'#!/bin/sh\nprintf verified\n'
        source.write_bytes(body)
        (self.parent / 'SHA256SUMS').write_text(hashlib.sha256(body).hexdigest() + '  install.sh\n')
        snapshot = verified_bootstrap(self.parent, self.root)
        source.write_bytes(b'#!/bin/sh\nprintf changed\n')
        self.assertEqual(subprocess.check_output(['sh', str(snapshot)]), b'verified')
        self.assertEqual(snapshot.stat().st_mode & 0o777, 0o700)
        next_work = self.root / 'next'; next_work.mkdir()
        with self.assertRaisesRegex(files.PublicationError, 'identity mismatch'):
            verified_bootstrap(self.parent, next_work)
        self.assertFalse((next_work / 'install.sh').exists())

    def test_directory_retry_repeats_ancestor_barriers(self):
        path = self.root / 'nested/child'
        for retry in [False, True]:
            observed = []
            with patch.object(files, 'sync_directory', side_effect=lambda p: observed.append(p)):
                files.create_directory(path, exist_ok=retry)
            self.assertEqual(observed, [path.absolute(), *path.absolute().parents])
        with patch.object(files, 'sync_directory', side_effect=OSError('required barrier')):
            with self.assertRaisesRegex(OSError, 'required barrier'):
                files.create_directory(path, exist_ok=True)

    def test_growing_frozen_copy_stops_before_writing_unbounded_input(self):
        source = self.parent / 'source'
        source.write_bytes(b'x' * (2 * 1024 * 1024))
        output = io.BytesIO()
        with self.assertRaisesRegex(ValueError, 'identity mismatch'):
            files.copy_stream(source, output, expected={'bytes': 8, 'sha256': '0' * 64})
        self.assertEqual(output.getvalue(), b'')

    def test_unlinked_preparation_stays_pinned_through_cleanup_identity_check(self):
        descriptor, names, checked = [], [], []
        original_check = files._Preparation.check_preparation
        def write(output):
            descriptor.append(output.fileno())
            output.write(b'private'); output.flush()
            name = next(self.parent.glob('.hive-tmp-*')); names.append(name)
            name.unlink()
            name.write_bytes(b'foreign')
            raise OSError('source failed')
        def check(preparation):
            # Keeping the unlinked inode open prevents a foreign entry from
            # acquiring its identity before this namespace decision.
            info = os.fstat(descriptor[0])
            self.assertEqual(info.st_nlink, 0)
            checked.append(info.st_ino)
            return original_check(preparation)
        with patch.object(files._Preparation, 'check_preparation', check):
            with self.assertRaises(files.PublicationError) as caught:
                files.atomic_stream(self.path, write)
        self.assertEqual(len(checked), 1)
        self.assertIn('source failed', str(caught.exception.__cause__))
        self.assertIsNotNone(caught.exception.retained)
        self.assertEqual(names[0].read_bytes(), b'foreign')
        self.assertFalse(self.path.exists())
        with self.assertRaises(OSError):
            os.fstat(descriptor[0])
