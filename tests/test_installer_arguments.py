"""Native release installer and dependency-free bootstrap argument contracts."""
import json
import hashlib
import os
import platform
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from release.files import atomic_write_text


class InstallerArguments(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.assets = self.root / 'assets'
        self.assets.mkdir()
        self.prefix = self.root / 'installation'
        self.marker = self.root / 'executed'
        tools = self.root / 'tools'
        tools.mkdir()
        for name in ['curl', 'cp', 'uname', 'mktemp']:
            atomic_write_text(tools / name, '#!/bin/sh\nprintf called >> "$MARKER"\nexit 71\n', mode=0o700)
        self.env = dict(os.environ, PATH=f'{tools}:/usr/bin:/bin', MARKER=str(self.marker))
        self.env.pop('HOME', None)
        self.binary = ROOT / 'target/release/hive-install'
        self.assertTrue(self.binary.is_file(), 'Build the release hive-install binary first')

    def run_cli(self, shell, args):
        command = ['sh', str(ROOT / 'packaging/install.sh')] if shell else [str(self.binary)]
        return subprocess.run(command + list(map(str, args)), env=self.env, capture_output=True, text=True, timeout=10, check=False)

    def test_invalid_input_precedes_transport_and_prefix_changes(self):
        base = ['--from', self.assets, '--prefix', self.prefix]
        cases = [
            ['--from', self.assets, '--prefix', 'relative'],
            ['--from', self.assets, '--prefix', ''],
            ['--from', self.assets, '--prefix'],
            ['--from', self.assets, '--prefix', '--setup-services=false', self.prefix],
            ['--from', '--prefix', self.prefix],
            ['--from', '', '--prefix', self.prefix],
            ['--from', self.assets],  # HOME absent and prefix omitted.
            base + ['--core', '--core'],
            base + ['--standalone-comb', '--core'],
            base + ['--no-setup', '--', '--yes'],
            base + ['--uninstall', '--', '--yes'],
            base + ['--uninstall', '--config', '/private/config'],
            base + ['--uninstall', '--no-setup'],
            base + ['--private-token=SECRET_SENTINEL'],
        ]
        for shell in [False, True]:
            for args in cases:
                with self.subTest(shell=shell, args=args):
                    reply = self.run_cli(shell, args)
                    self.assertEqual(reply.returncode, 2, reply.stderr)
                    error = json.loads(reply.stdout)['error']
                    for field in ['code', 'message', 'field', 'expected', 'examples']:
                        self.assertTrue(error[field], (field, error))
                    self.assertIn('Examples:', reply.stderr)
                    self.assertNotIn('SECRET_SENTINEL', reply.stdout + reply.stderr)
                    self.assertFalse(self.marker.exists(), 'Invalid input ran a transport/platform helper')
                    self.assertFalse(self.prefix.exists(), 'Invalid input created the installation')

    def test_invalid_https_host_never_reaches_transport(self):
        for url in ['https://:443', 'https://?secret', 'https://user@:443', 'https://[]', 'https://[::1', 'https://']:
            with self.subTest(url=url):
                reply = self.run_cli(True, ['--from', url, '--prefix', self.prefix])
                self.assertEqual(reply.returncode, 2, reply.stderr)
                self.assertEqual(json.loads(reply.stdout)['error']['field'], '--from')
                self.assertFalse(self.marker.exists())
                self.assertFalse(self.prefix.exists())

    def test_valid_https_hosts_reach_the_platform_probe(self):
        for url in ['https://assets.example:443/releases', 'https://[::1]/releases', 'https://[::1]:443/releases']:
            with self.subTest(url=url):
                reply = self.run_cli(True, ['--from', url, '--prefix', self.prefix])
                self.assertEqual(reply.returncode, 71, reply.stderr)
                self.assertEqual(self.marker.read_text(), 'called')
                self.marker.unlink()
                self.assertFalse(self.prefix.exists())

    def test_help_with_no_home_has_examples_and_no_effects(self):
        for shell in [False, True]:
            for flag in ['-h', '--help']:
                with self.subTest(shell=shell, flag=flag):
                    reply = self.run_cli(shell, [flag])
                    self.assertEqual(reply.returncode, 0, reply.stderr)
                    self.assertIn('Examples:', reply.stdout)
                    self.assertIn('--prefix', reply.stdout)
                    self.assertFalse(self.marker.exists())
                    self.assertFalse(self.prefix.exists())

    def test_path_rejections_precede_helpers_and_source_asset_errors_name_the_asset(self):
        link = self.root / 'link'
        link.symlink_to(self.assets, target_is_directory=True)
        file = self.root / 'file'
        atomic_write_text(file, 'not a directory')
        for shell in [False, True]:
            for prefix in [link, str(link) + '/', str(link) + '/.', file / 'child', self.root / '..' / 'bad']:
                reply = self.run_cli(shell, ['--from', self.assets, '--prefix', prefix])
                self.assertEqual(reply.returncode, 2, reply.stderr)
                self.assertFalse(self.marker.exists())
            reply = self.run_cli(shell, ['--from', self.root / 'absent', '--prefix', self.prefix])
            self.assertEqual(reply.returncode, 2, reply.stderr)
            self.assertFalse(self.marker.exists())
        # Native runtime failure happens after valid input, before prefix publication.
        reply = self.run_cli(False, ['--from', self.assets, '--prefix', self.prefix, '--no-setup'])
        self.assertEqual(reply.returncode, 1, reply.stderr)
        self.assertIn('SHA256SUMS', reply.stderr)
        self.assertFalse(self.prefix.exists())

    def test_verified_bootstrap_preserves_original_setup_arguments(self):
        arch = 'aarch64' if platform.machine() in ['arm64', 'aarch64'] else 'x86_64'
        target = arch + ('-apple-darwin' if platform.system() == 'Darwin' else '-unknown-linux-gnu')
        helper = f'hive-install-{target}'
        assets = {
            helper: '#!/usr/bin/env python3\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n',
            helper + '.LICENSES': 'fixture notices',
            f'hive-{target}.tar.gz': 'transport-only fixture; native helper validates archives',
        }
        for name, body in assets.items():
            atomic_write_text(self.assets / name, body)
        sums = ''.join(f'{hashlib.sha256(body.encode()).hexdigest()}  {name}\n' for name, body in assets.items())
        atomic_write_text(self.assets / 'SHA256SUMS', sums)
        args = ['--from', str(self.assets), '--prefix', str(self.prefix), '--setup-provider', 'codex,claude',
                '--setup-services=false', '--', '--bridge-file', 'space $literal `name`.json']
        env = self.env | {'PATH': os.environ['PATH'], 'TMPDIR': str(self.root)}
        result = subprocess.run(['sh', str(ROOT / 'packaging/install.sh'), *args], env=env, capture_output=True, text=True, timeout=15, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        forwarded = json.loads(result.stdout)
        self.assertEqual(forwarded[0], '--staged-from')
        self.assertEqual(forwarded[2:], args)
        self.assertFalse(Path(forwarded[1]).exists(), 'Bootstrap did not clean its private staging')
        self.assertFalse(self.prefix.exists())

    def bootstrap_fixture(self, script, *, bad_cleanup=False):
        arch = 'aarch64' if platform.machine() in ['arm64', 'aarch64'] else 'x86_64'
        target = arch + ('-apple-darwin' if platform.system() == 'Darwin' else '-unknown-linux-gnu')
        helper = f'hive-install-{target}'
        assets = {helper: '#!/bin/sh\n' + script + '\n', helper + '.LICENSES': 'notices',
                  f'hive-{target}.tar.gz': 'transport fixture'}
        for name, body in assets.items():
            atomic_write_text(self.assets / name, body)
        atomic_write_text(self.assets / 'SHA256SUMS', ''.join(
            f'{hashlib.sha256(body.encode()).hexdigest()}  {name}\n' for name, body in assets.items()))
        path = os.environ['PATH']
        if bad_cleanup:
            tools = self.root / 'cleanup-tools'
            tools.mkdir()
            atomic_write_text(tools / 'rm', '#!/bin/sh\nprintf SECRET_SENTINEL >&2\nexit 1\n', mode=0o700)
            path = f'{tools}:{path}'
        return subprocess.run(['sh', str(ROOT / 'packaging/install.sh'), '--from', str(self.assets),
                               '--prefix', str(self.prefix), '--no-setup'],
                              env=self.env | {'PATH': path, 'TMPDIR': str(self.root)},
                              capture_output=True, text=True, timeout=15, check=False)

    def test_cleanup_failure_preserves_original_helper_exit(self):
        reply = self.bootstrap_fixture('exit 42', bad_cleanup=True)
        self.assertEqual(reply.returncode, 42, reply.stderr)
        self.assertIn('bootstrap.cleanup_failed', reply.stderr)
        self.assertNotIn('SECRET_SENTINEL', reply.stderr)
        self.assertFalse(self.prefix.exists())

    def test_signal_is_preserved_after_staging_cleanup(self):
        reply = self.bootstrap_fixture('kill -TERM "$PPID"\nexit 0')
        self.assertEqual(reply.returncode, -15, reply.stderr)
        self.assertIn('bootstrap.signal', reply.stderr)
        self.assertEqual(list(self.root.glob('hive-install.*')), [])
        self.assertFalse(self.prefix.exists())


if __name__ == '__main__':
    unittest.main()
