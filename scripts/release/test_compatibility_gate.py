"""Synthetic contracts exercise freeze enforcement without declaring a release."""
import copy
from contextlib import redirect_stderr
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from . import compatibility_gate as gate
from .files import atomic_write_json


class CompatibilityGate(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'scripts/release').mkdir(parents=True)
        self.row = dict(name='state', current=1, minimum=1, maximum=1, released=[])
        self.artifact = dict(id='record', path='record.json', encoding='json', test='native_record_contract')
        self.inventory = dict(
            registries=[dict(crate='fixture', path='registry.json', test='native_registry')],
            contracts=[dict(crate='fixture', name='state', artifacts=[self.artifact], remaining_coverage=[])],
        )
        atomic_write_json(self.root / 'record.json', {'format': 1, 'body': 'original'})
        self.save()

    def save(self):
        atomic_write_json(self.root / 'registry.json', [self.row])
        atomic_write_json(self.root / gate.MANIFEST, self.inventory)

    def freeze(self):
        digest = gate.fingerprint(self.root, [self.artifact])
        self.row['released'] = [dict(product_version='1.0.0', number=1, fingerprint=digest)]
        self.save()

    def test_draft_can_change_but_released_number_is_frozen(self):
        self.assertEqual(len(gate.check(self.root)), 1)
        self.freeze()
        gate.check(self.root)
        atomic_write_json(self.root / 'record.json', {'format': 1, 'body': 'changed'})
        with self.assertRaisesRegex(ValueError, 'bump the number'):
            gate.check(self.root)
        self.row.update(current=2, maximum=2)
        self.save()
        gate.check(self.root)

    def test_rewriting_or_removing_released_history_is_refused(self):
        self.freeze()
        before = {'state': copy.deepcopy(self.row)}
        for change in ('fingerprint', 'removal', 'crate-removal'):
            current = copy.deepcopy(before)
            if change == 'fingerprint':
                current['state']['released'][0]['fingerprint'] = '0' * 64
            elif change == 'removal':
                current['state']['released'] = []
            else:
                current = {}
            with self.subTest(change=change), self.assertRaises(ValueError):
                gate.preserve_releases(current, before)

    def test_partial_coverage_warns_but_freezes_without_weakening_immutability(self):
        self.inventory['contracts'][0]['remaining_coverage'] = ['a missing variant', 'another uncovered record']
        self.save()
        with redirect_stderr(io.StringIO()) as draft:
            self.assertEqual(len(gate.check(self.root)), 1)
        self.assertIn('WARNING: incomplete compatibility coverage for fixture/state:', draft.getvalue())
        self.freeze()
        with redirect_stderr(io.StringIO()) as frozen:
            self.assertEqual(len(gate.check(self.root)), 1)
        self.assertEqual(frozen.getvalue(), draft.getvalue())
        for gap in self.inventory['contracts'][0]['remaining_coverage']:
            self.assertIn(gap, frozen.getvalue())
        atomic_write_json(self.root / 'record.json', {'format': 1, 'body': 'changed'})
        with redirect_stderr(io.StringIO()), self.assertRaisesRegex(ValueError, 'bump the number'):
            gate.check(self.root)

    def test_missing_duplicate_or_orphan_contracts_are_refused(self):
        original = copy.deepcopy(self.inventory)
        for mode in ('missing', 'duplicate', 'orphan'):
            self.inventory = copy.deepcopy(original)
            if mode == 'missing':
                self.inventory['contracts'] = []
            elif mode == 'duplicate':
                self.inventory['contracts'] *= 2
            else:
                self.inventory['contracts'][0]['name'] = 'unknown'
            self.save()
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                gate.check(self.root)

    def test_json_is_canonical_but_bytes_and_artifact_ids_are_exact(self):
        first = gate.fingerprint(self.root, [self.artifact])
        atomic_write_json(self.root / 'record.json', {'body': 'original', 'format': 1})
        self.assertEqual(gate.fingerprint(self.root, [self.artifact]), first)
        changed = dict(self.artifact, id='renamed')
        self.assertNotEqual(gate.fingerprint(self.root, [changed]), first)
        changed['encoding'] = 'bytes'
        self.assertNotEqual(gate.fingerprint(self.root, [changed]), first)
        for raw in ('{"name":1,"name":2}', '{"value":NaN}'):
            with self.assertRaises(ValueError):
                gate.decode(raw)
        with self.assertRaises(ValueError):
            gate.path(self.root, '../foreign')

    def test_registry_numbers_and_fingerprints_are_strict(self):
        for key, value in [('current', True), ('minimum', 0), ('maximum', '1')]:
            bad = dict(self.row, **{key: value})
            with self.assertRaises(ValueError):
                gate.registry(json.dumps([bad]))
        with self.assertRaises(ValueError):
            gate.registry(json.dumps([self.row, self.row]))

    def test_whole_change_baseline_keeps_history_removed_in_an_earlier_commit(self):
        def git(*args):
            return subprocess.check_output(
                ['git', '-c', 'user.name=Compatibility fixture', '-c',
                 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', *args],
                cwd=self.root, stderr=subprocess.STDOUT, text=True).strip()

        git('init')
        self.freeze()
        git('add', '.')
        git('commit', '-m', 'Synthetic released fixture')
        before = git('rev-parse', 'HEAD')
        self.row['released'] = []
        self.save()
        git('add', '.')
        git('commit', '-m', 'Remove frozen history')
        atomic_write_json(self.root / 'record.json', {'body': 'changed'})
        git('add', '.')
        git('commit', '-m', 'Change draft shape')
        # Looking only one commit back loses the release deletion in this push.
        gate.check(self.root, 'HEAD^')
        with self.assertRaisesRegex(ValueError, 'removed or rewritten'):
            gate.check(self.root, before)
        with self.assertRaises(subprocess.CalledProcessError):
            gate.baseline_commit(self.root, '--help')

    def test_released_corpus_pins_cannot_be_rewritten(self):
        from unittest.mock import patch
        (self.root / 'compat').mkdir()
        row = {'version': '1.0.0', 'target': 'fixture', 'corpus_sha256': 'a' * 64}
        with patch.object(gate, 'baseline_file', return_value=json.dumps([row]).encode()):
            atomic_write_json(self.root / 'compat/releases.json', [row])
            gate.preserve_corpora(self.root, 'baseline')
            for current in ([], [dict(row, corpus_sha256='b' * 64)]):
                atomic_write_json(self.root / 'compat/releases.json', current)
                with self.assertRaisesRegex(ValueError, 'removed or rewritten'):
                    gate.preserve_corpora(self.root, 'baseline')
