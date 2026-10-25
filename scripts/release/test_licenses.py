"""Vendored dependency notices must survive the normal-graph selection."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from . import licenses


class LicenseTests(unittest.TestCase):
    def test_path_dependency_is_included_but_exact_first_party_is_excluded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            vendor = root / 'vendor/widget'
            vendor.mkdir(parents=True)
            (vendor / 'LICENSE').write_text('Third-party widget permission notice.\n')
            docs = root / 'toolchain/share/doc/rust'
            (docs / 'licenses').mkdir(parents=True)
            for name in ['COPYRIGHT-library.html', 'licenses/MIT.txt', 'licenses/Apache-2.0.txt']:
                (docs / name).write_text('Toolchain notice.\n')
            entries = [
                dict(name='hive', version='0.1.0', source=None, manifest_path=str(root / 'Cargo.toml')),
                dict(name='widget', version='1.0.0', source=None, license='MIT',
                     manifest_path=str(vendor / 'Cargo.toml')),
            ]

            def run(args):
                if args[:2] == ['cargo', 'tree']:
                    return 'hive v0.1.0\nwidget v1.0.0\n'
                if args[:2] == ['cargo', 'metadata']:
                    return json.dumps({'packages': entries})
                if args == ['rustc', '--print', 'sysroot']:
                    return str(root / 'toolchain')
                self.assertEqual(args, ['rustc', '-V'])
                return 'rustc fixture'

            with patch.object(licenses, 'ROOT', root), patch.object(licenses, 'run', run):
                records, text = licenses.collect('hive', [], 'fixture')
                self.assertEqual([r['name'] for r in records], ['widget', 'rust-standard-library'])
                self.assertIn('Third-party widget permission notice.', text)
                self.assertIn('path dependency from the build checkout', text)
                self.assertNotIn('crates.io/api/v1/crates/widget', text)
                self.assertFalse(licenses.first_party(dict(entries[0], manifest_path=str(vendor / 'Cargo.toml'))))
                (vendor / 'LICENSE').unlink()
                with self.assertRaisesRegex(ValueError, 'Missing license text: widget'):
                    licenses.collect('hive', [], 'fixture')


if __name__ == '__main__':
    unittest.main()
