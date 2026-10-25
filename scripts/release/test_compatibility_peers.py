import copy
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from .compatibility_peers import catalog, declarations, materialize
from .compatibility_recipe import recipe, previous_line
from .compatibility_skew import overlap, outcome
from subprocess import CompletedProcess
from .files import atomic_write_json, create_directory


def archive(names, *, kind=tarfile.REGTYPE):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:gz') as tar:
        for name in names:
            body = name.encode()
            info = tarfile.TarInfo('bin/' + name)
            info.type, info.size = kind, len(body)
            tar.addfile(info, io.BytesIO(body))
    return output.getvalue()


class ReleasePeerTests(unittest.TestCase):
    def setUp(self):
        self.row = {'corpus_sha256': 'a' * 64, 'version': '1.0.0', 'target': 'aarch64-apple-darwin', 'parts': {}}
        self.assets = {}
        for part, names in [('hive', ['hivectl', 'hive-daemon']), ('comb', ['comb'])]:
            data = archive(names)
            url = f'https://github.com/metacircu1ar/hive/releases/download/v1.0.0/{part}.tar.gz'
            self.assets[url] = data
            self.row['parts'][part] = {'url': url, 'sha256': hashlib.sha256(data).hexdigest(), 'binaries': {
                name: {'member': 'bin/' + name, 'sha256': hashlib.sha256(name.encode()).hexdigest()} for name in names}}

    def test_empty_released_set_is_explicit_and_fixture_downloads_are_pinned(self):
        self.assertEqual(catalog('[]'), [])
        row, = catalog(json.dumps([self.row]))
        with tempfile.TemporaryDirectory() as temp:
            destination = Path(temp) / 'release'
            materialize(row, destination, self.assets.__getitem__)
            self.assertEqual(sorted(p.name for p in destination.iterdir()), ['comb', 'hive-daemon', 'hivectl'])
            self.assertEqual((destination / 'hivectl').read_bytes(), b'hivectl')
            self.assertEqual((destination / 'hivectl').stat().st_mode & 0o777, 0o700)
            with self.assertRaises(FileExistsError):
                materialize(row, destination, self.assets.__getitem__)

    def test_checksum_duplicates_links_and_member_substitution_are_refused(self):
        for mode in ['archive-hash', 'binary-hash', 'duplicate', 'symlink']:
            row = copy.deepcopy(self.row)
            assets = dict(self.assets)
            part = row['parts']['hive']
            if mode == 'archive-hash':
                part['sha256'] = '0' * 64
            elif mode == 'binary-hash':
                part['binaries']['hivectl']['sha256'] = '0' * 64
            else:
                data = archive(['hivectl', 'hivectl', 'hive-daemon']) if mode == 'duplicate' else archive(['hivectl', 'hive-daemon'], kind=tarfile.SYMTYPE)
                assets[part['url']] = data
                part['sha256'] = hashlib.sha256(data).hexdigest()
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp, self.assertRaises(ValueError):
                materialize(row, Path(temp) / 'release', assets.__getitem__)
        for change in ['url', 'hash', 'member', 'version']:
            row = copy.deepcopy(self.row)
            if change == 'url': row['parts']['hive']['url'] = 'https://example.invalid/file'
            elif change == 'hash': row['parts']['hive']['sha256'] = None
            elif change == 'member': row['parts']['hive']['binaries']['hivectl']['member'] = 'bin/comb'
            else: row['version'] = '../foreign'
            with self.subTest(change=change), self.assertRaises(ValueError):
                catalog(json.dumps([row]))
        with self.assertRaises(ValueError):
            catalog(json.dumps([self.row, self.row]))

    def test_declared_releases_cannot_be_skipped_or_invented(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            create_directory(root / 'scripts/release')
            atomic_write_json(root / 'scripts/release/compatibility.json', {'registries': [{'path': 'registry.json'}]})
            atomic_write_json(root / 'registry.json', [{'released': []}])
            declarations(root, [], self.row['target'])
            with self.assertRaises(ValueError):
                declarations(root, [self.row], self.row['target'])
            atomic_write_json(root / 'registry.json', [{'released': [{'product_version': '1.0.0'}]}])
            with self.assertRaises(ValueError):
                declarations(root, [], self.row['target'])
            with self.assertRaises(FileNotFoundError):
                declarations(root, [self.row], self.row['target'])
            create_directory(root / 'compat/1.0.0')
            atomic_write_json(root / 'compat/1.0.0/corpus.json', {'kind': 'released', 'product_version': '1.0.0'})
            self.row['corpus_sha256'] = hashlib.sha256((root / 'compat/1.0.0/corpus.json').read_bytes()).hexdigest()
            with self.assertRaises(ValueError):
                declarations(root, [self.row], self.row['target'])
            self.row['corpus_sha256'] = freeze_recipe(root)
            declarations(root, [self.row], self.row['target'])
            with self.assertRaises(ValueError):
                declarations(root, [self.row], 'x86_64-unknown-linux-gnu')

    def test_synthetic_and_released_recipes_load_identical_pinned_inputs(self):
        source = Path(__file__).resolve().parents[2]
        synthetic = recipe(source, None, synthetic=True)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            pin = freeze_recipe(root)
            frozen = recipe(root, '1.0.0', expected_sha256=pin)
            self.assertEqual(synthetic, frozen)
            body = json.loads(frozen['scenario'])
            self.assertEqual(body['output']['repetitions'], 32768)
            self.assertEqual(len(body['queued']), 3)
            selected = root / 'compat/1.0.0/scenario'
            atomic_write_json(selected, dict(body, queued=['replaced']))
            with self.assertRaisesRegex(ValueError, 'fixture checksum'):
                recipe(root, '1.0.0', expected_sha256=pin)
            with self.assertRaisesRegex(ValueError, 'recipe checksum'):
                recipe(root, '1.0.0', expected_sha256='0' * 64)

    def test_previous_line_does_not_require_permanent_skew_support(self):
        versions = ['1.0.0', '1.1.0', '1.1.1', '1.2.0']
        self.assertEqual(previous_line(versions, '1.3.0'), {'1.2.0'})
        self.assertEqual(previous_line(versions, '1.2.1'), {'1.1.0', '1.1.1', '1.2.0'})
        self.assertEqual(previous_line(versions, '2.0.0'), {'1.2.0'})
        self.assertFalse(overlap({'minimum': 1, 'maximum': 1}, {'minimum': 2, 'maximum': 3}))
        self.assertTrue(overlap({'minimum': 1, 'maximum': 2}, {'minimum': 2, 'maximum': 3}))
        rejected = CompletedProcess([], 1, json.dumps({'error': {'code': 'incompatible_protocol', 'message': 'Upgrade Hive CLI'}}), '')
        self.assertEqual(outcome(rejected, False, 'Hive'), 'incompatible_protocol')
        with self.assertRaises(RuntimeError):
            outcome(rejected, False, 'Comb')
        accepted = CompletedProcess([], 0, '{}', '')
        self.assertEqual(outcome(accepted, True, 'Hive'), 'shared_protocol')
        with self.assertRaises(RuntimeError):
            outcome(accepted, False, 'Hive')


def freeze_recipe(root):
    source = Path(__file__).resolve().parents[2]
    value = json.loads((source / 'compat/synthetic-0/corpus.json').read_bytes())
    value.update(kind='released', product_version='1.0.0')
    directory = root / 'compat/1.0.0'
    create_directory(directory, exist_ok=True)
    from .files import atomic_write_bytes
    for name, pin in value['fixtures'].items():
        data = (source / pin['path']).read_bytes()
        pin['path'] = 'compat/1.0.0/' + name
        atomic_write_bytes(root / pin['path'], data)
    atomic_write_json(directory / 'corpus.json', value)
    return hashlib.sha256((directory / 'corpus.json').read_bytes()).hexdigest()
