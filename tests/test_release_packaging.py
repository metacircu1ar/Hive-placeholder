"""Release artifact provenance and native dependency boundaries, without builds."""
import hashlib
import json
import os
import shutil
import subprocess
import struct
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from package_release import package
from release.acceptance import Cli
from release.common import inspect_binary
from release.docs import GUIDES, validate_links
from release.policy import PACKAGES, allowed_path, check_product_text, check_text, check_tracked, rust_text


class ProductBoundaryTests(unittest.TestCase):
    def test_installed_cli_uses_explicit_prefixes_for_consumers(self):
        remote = ['hivectl', '--state-dir', '/state']
        control = remote + ['--config', '/config']
        call = Cli(remote, prefixes={'daemon': control, 'auth': control})
        for args, prefix in [(('spawn', '--body', 'work'), remote),
                             (('evidence', 'correction', '--summary', 'fact'), remote),
                             (('collectors', 'status'), remote),
                             (('daemon', 'status'), control),
                             (('daemon', 'stop'), control),
                             (('auth', 'list'), control)]:
            self.assertEqual(call.argv(*args), prefix + list(args))
        self.assertEqual(remote, ['hivectl', '--state-dir', '/state'])

    def test_current_product_text_and_tracked_categories(self):
        check_product_text(ROOT)
        check_tracked(ROOT)

    def test_internal_material_cannot_enter_a_product_category(self):
        for name in ('docs/new-plan.md', 'IMPL_PLAN.md', 'scripts/run_demo.py',
                     'examples/generate_icons.rs', 'icons/generated/parallax.png',
                     'comb/models/helper.py', 'tests/test_scale.py',
                     'src/notes.md', '../outside.rs',
                     *(f'scripts/release/{prefix}{gate}.py' for prefix in ('', 'test_')
                       for gate in ('error_gate', 'write_gate', 'log_gate', 'python_error_gate')),
                     *(f'scripts/release/{name}.json' for name in
                       ('error-sites', 'write-sites', 'python-errors', 'log-canaries'))):
            with self.subTest(name=name):
                self.assertFalse(allowed_path(name))
        for name in ('setup-support/src/new.rs', 'tests/runtime/new.rs',
                     'tests/fixtures/pty_setup.py', 'examples/qwen3-embedding.json',
                     'logo.png', 'comb/logo.png', 'icons/generated/hive-64.data-uri', 'scripts/release/policy.py'):
            with self.subTest(name=name):
                self.assertTrue(allowed_path(name))

    def test_prose_policy_rejects_internal_text_including_code_examples(self):
        for text in ('Parallax', 'scenarios', 'benchmark', 'historical', 'research',
                     'validation leg', 'target/release/hivectl', 'cargo test --release',
                     'cargo clippy', 'cargo fmt', 'B16', 'V2', 'P3', 'unreleased',
                     'run_live_comb.py', 'scripts/run_example.py',
                     'scenario-runs/output', 'dist/assets/install.sh'):
            for body in (text, f'```sh\n{text}\n```'):
                with self.subTest(body=body), self.assertRaises(ValueError):
                    check_text('guide.md', body, markdown=True)
        with self.assertRaisesRegex(ValueError, 'Internal date'):
            check_text('guide.md', 'Updated on 2026-10-13.', markdown=True)
        check_text('guide.md', 'Use `2026-10-13` as the date.\n\n```json\n{"at":"2026-10-13"}\n```', markdown=True)
        check_text('guide.md', 'Use /v1/embeddings and e5-small-v2; validation errors stop a request.')

    def test_rust_text_includes_help_attributes_docs_raw_and_continued_strings(self):
        source = '// internal benchmark comment\n/// Run persistent agents.\n'
        source += '#[command(about = "Run \\\n   agents")]\nconst S: &str = r##"Read \\"facts\\"."##;'
        texts = [text for text, _ in rust_text(source)]
        self.assertEqual(texts, [' Run persistent agents.', 'Run agents', r'Read \"facts\".'])

    def test_embedded_help_instructions_and_cargo_description_are_enforced(self):
        with tempfile.TemporaryDirectory() as temp:
            # A checkout's ancestor name must not disable embedded-text checks.
            root = Path(temp) / 'tests' / 'checkout'
            for name in ('Cargo.toml', *(f'{package}/Cargo.toml' for package in PACKAGES),
                         'packaging/install.sh'):
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text('')
            cases = (
                ('src/cli/new.rs', '/// P3 setup\n'),
                ('src/cli/new.rs', '/** B16 setup help */\n'),
                ('src/runtime/api/tools.rs', 'const HELP: &str = r#"research tools"#;'),
                ('src/runtime/api/request.rs', '/// B16 tool field\n'),
                ('comb/src/setup.rs', '/// Unreleased setup\n'),
                ('src/external_mcp/catalog.rs', 'const HELP: &str = "Updated 2026-10-13";'),
                ('src/runtime/worker/session/startup.rs', 'const HELP: &str = "scripts/run_demo.py";'),
                ('comb/src/provision/serve.rs', 'const HELP: &str = "run_live_comb.py";'),
                ('installer/src/options.rs', '#[command(about = "benchmark installer")]'),
                ('Cargo.toml', 'description = "Unreleased tool"'),
                ('packaging/install.sh', "echo 'scenario runner'"),
            )
            with patch('release.policy.product_files', return_value={}):
                for name, source in cases:
                    path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text(source)
                    with self.subTest(name=name), self.assertRaises(ValueError):
                        check_product_text(root)
                    path.write_text('')


class ReleasePackagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.build = self.root / 'freeze'; (self.build / 'release').mkdir(parents=True)
        self.output = self.root / 'assets'
        deps = {'libraries': ['/usr/lib/libSystem.B.dylib'], 'required_glibc': None}
        binaries = {}
        for name in ['hivectl', 'hive-daemon']:
            data = ('synthetic fixture ' + name).encode(); (self.build / 'release' / name).write_bytes(data)
            binaries[name] = {'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data), 'dependencies': deps, 'product_version': '0.1.0'}
        notices = b'fixture third-party notices'; (self.build / 'THIRD_PARTY_LICENSES').write_bytes(notices)
        self.record = {'part': 'hive', 'target': 'aarch64-apple-darwin', 'profile': 'release',
                       'command': ['cargo', 'build', '--release', '--locked', '-p', 'hive', '--message-format=json-render-diagnostics'],
                       'binaries': binaries, 'notices_sha256': hashlib.sha256(notices).hexdigest(), 'notices_bytes': len(notices)}
        self.save()
        self.addCleanup(patch.stopall)
        patch('package_release.native_target', return_value=self.record['target']).start()
        patch('package_release.inspect_binary', return_value=deps).start()

    def save(self):
        (self.build / 'build.json').write_text(json.dumps(self.record))

    def test_plain_record_archives_exact_files_with_notices_and_skill_reference(self):
        package(self.build, self.output)
        path = self.output / 'hive-aarch64-apple-darwin.tar.gz'
        with tarfile.open(path) as archive:
            manifest = json.load(archive.extractfile('manifest.json'))
            self.assertEqual(manifest['format'], 1)
            self.assertEqual(manifest['product_version'], '0.1.0')
            self.assertEqual(set(archive.getnames()), set(manifest['files']) | {'manifest.json'})
            self.assertIn('share/hive/docs/cli-reference.md', manifest['files'])
            self.assertIn('share/hive/skills/hive/SKILL.md', manifest['files'])
            self.assertIn('share/hive/THIRD_PARTY_LICENSES', manifest['files'])
        self.assertIn(hashlib.sha256(path.read_bytes()).hexdigest(), (self.output / 'SHA256SUMS').read_text())

    def test_standalone_comb_carries_complete_product_guides(self):
        self.record['part'] = 'comb'
        self.record['command'][5] = 'comb'
        self.record['binaries'] = {'comb': self.record['binaries']['hivectl']}
        (self.build / 'release/comb').write_bytes((self.build / 'release/hivectl').read_bytes())
        self.save()
        package(self.build, self.output)
        with tarfile.open(self.output / 'comb-aarch64-apple-darwin.tar.gz') as archive:
            prefix = 'share/comb/'
            files = {n[len(prefix):]: archive.extractfile(n).read()
                     for n in archive.getnames() if n.startswith(prefix)}
        self.assertTrue(set(GUIDES).issubset(files))
        self.assertIn('examples/qwen3-embedding.json', files)
        self.assertIn('examples/comb.yaml', files)
        self.assertNotIn('AGENTS.md', files)
        self.assertFalse(any('benchmarks/results' in name or name.endswith('-plan.md') for name in files))
        validate_links(files)

    def test_product_guides_reject_missing_links_and_internal_workspace(self):
        for body in [b'[missing](missing.md)', b'<img src="missing.png">',
                     b'[reference]: missing.md']:
            with self.assertRaisesRegex(ValueError, 'Unbundled documentation link'):
                validate_links({'README.md': body})
        for body in [b'[internal](https://github.com/metacircu1ar/parallax/blob/main/README.md)',
                     b'[internal](../parallax/docs/README.md)']:
            with self.assertRaisesRegex(ValueError, 'Internal workspace reference'):
                validate_links({'README.md': body})
        validate_links({'README.md': b'[local](guide.md#commands)', 'guide.md': b'# Commands'})

    def test_changed_binary_or_notices_refused_before_output(self):
        (self.build / 'release/hivectl').write_text('replaced by cargo test')
        with self.assertRaisesRegex(ValueError, 'identity mismatch'):
            package(self.build, self.output)
        self.assertFalse(self.output.exists())

    def test_test_or_debug_build_record_is_not_a_shipping_build(self):
        for command in [['cargo', 'test', '--release', '--locked', '-p', 'hive'], ['cargo', 'build', '--locked', '-p', 'hive']]:
            self.record['command'] = command; self.save()
            with self.assertRaisesRegex(ValueError, 'plain package-scoped release'):
                package(self.build, self.output)

    def test_missing_or_extra_binary_cannot_claim_complete_pair(self):
        del self.record['binaries']['hive-daemon']; self.save()
        with self.assertRaisesRegex(ValueError, 'wrong binary set'):
            package(self.build, self.output)

    def helper_build(self):
        self.record['part'] = 'hive-install'
        self.record['command'][5] = 'hive-install'
        self.record['binaries'] = {'hive-install': self.record['binaries']['hivectl']}
        (self.build / 'release/hive-install').write_bytes((self.build / 'release/hivectl').read_bytes())
        self.save()

    def test_consumed_archive_and_helper_bytes_must_match_frozen_preflight(self):
        import package_release
        from release.files import PublicationError
        for helper in (False, True):
            if helper:
                self.helper_build()
            binary = 'hive-install' if helper else 'hivectl'
            source = self.build / 'release' / binary
            original = source.read_bytes()
            real_publish = package_release.publish
            def mutate(*args):
                source.write_bytes(bytes([original[0] ^ 1]) + original[1:])
                return real_publish(*args)
            with self.subTest(helper=helper), patch('package_release.publish', side_effect=mutate):
                with self.assertRaisesRegex(PublicationError, 'identity mismatch'):
                    package(self.build, self.output)
            name = 'hive-install-aarch64-apple-darwin' if helper else 'hive-aarch64-apple-darwin.tar.gz'
            self.assertFalse((self.output / name).exists())
            self.assertFalse((self.output / 'SHA256SUMS').exists())
            source.write_bytes(original)

    def test_changed_notice_after_preflight_cannot_be_advertised(self):
        import package_release
        from release.files import PublicationError
        notice = self.build / 'THIRD_PARTY_LICENSES'
        original = notice.read_bytes()
        real_publish = package_release.publish
        def mutate(*args):
            notice.write_bytes(b'x' * len(original))
            return real_publish(*args)
        with patch('package_release.publish', side_effect=mutate):
            with self.assertRaisesRegex(PublicationError, 'identity mismatch'):
                package(self.build, self.output)
        self.assertFalse((self.output / 'hive-aarch64-apple-darwin.tar.gz').exists())
        self.assertFalse((self.output / 'SHA256SUMS').exists())

    def test_failed_checksum_commit_can_retry_complete_assets(self):
        from release import files
        real_write = files.atomic_write_text
        def fail(path, *args, **kwargs):
            if path.name == 'SHA256SUMS':
                raise OSError('checksum fault')
            return real_write(path, *args, **kwargs)
        with patch.object(files, 'atomic_write_text', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'checksum fault'):
                package(self.build, self.output)
        archive = self.output / 'hive-aarch64-apple-darwin.tar.gz'
        with tarfile.open(archive) as contents:
            self.assertIn('manifest.json', contents.getnames())
        self.assertFalse((self.output / 'SHA256SUMS').exists())
        package(self.build, self.output)
        for line in (self.output / 'SHA256SUMS').read_text().splitlines():
            digest, name = line.split()
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)
        self.assertEqual((self.output / 'install.sh').stat().st_mode & 0o777, 0o755)
        self.assertEqual(archive.stat().st_mode & 0o777, 0o644)
        self.assertEqual((self.output / 'SHA256SUMS').stat().st_mode & 0o777, 0o644)

    def test_two_package_processes_preserve_checksum_union(self):
        other = self.root / 'other'; shutil.copytree(self.build, other)
        record = json.loads((other / 'build.json').read_bytes())
        record['part'] = 'comb'; record['command'][5] = 'comb'
        record['binaries'] = {'comb': record['binaries']['hivectl']}
        (other / 'release/comb').write_bytes((other / 'release/hivectl').read_bytes())
        (other / 'build.json').write_text(json.dumps(record))
        script = """import json, sys
from pathlib import Path
import package_release as p
build, output = map(Path, sys.argv[1:])
record = json.loads((build/'build.json').read_bytes())
p.native_target = lambda: record['target']
p.inspect_binary = lambda *_: next(iter(record['binaries'].values()))['dependencies']
p.package(build, output)
"""
        env = dict(os.environ, PYTHONPATH=str(ROOT / 'scripts'))
        processes = []
        try:
            for build in [self.build, other]:
                processes.append(subprocess.Popen([sys.executable, '-c', script, str(build), str(self.output)],
                                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            for process in processes:
                stdout, stderr = process.communicate(timeout=20)
                self.assertEqual(process.returncode, 0, (stdout, stderr))
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill(); process.wait(timeout=5)
        sums = dict((name, digest) for digest, name in
                    (line.split() for line in (self.output / 'SHA256SUMS').read_text().splitlines()))
        for part in ['hive', 'comb']:
            name = f'{part}-aarch64-apple-darwin.tar.gz'
            self.assertEqual(sums[name], hashlib.sha256((self.output / name).read_bytes()).hexdigest())
        self.assertFalse(list(self.output.glob('.hive-tmp-*')))

    def test_native_audit_rejects_wrong_architecture_and_external_libraries(self):
        binary = self.root / 'native'
        binary.write_bytes(b'\xcf\xfa\xed\xfe' + struct.pack('<I', 0x100000C) + bytes(24))
        with self.assertRaisesRegex(ValueError, 'architecture'):
            inspect_binary(binary, 'x86_64-apple-darwin')
        with patch('release.common.run', return_value='native:\n /tmp/build/libcustom.dylib (compatibility version 1.0.0)'):
            with self.assertRaisesRegex(ValueError, 'Non-system shared library'):
                inspect_binary(binary, 'aarch64-apple-darwin')

    def test_linux_records_glibc_and_refuses_the_cpp_runtime(self):
        binary = self.root / 'elf'
        header = bytearray(32); header[:6] = b'\x7fELF\x02\x01'; struct.pack_into('<H', header, 18, 62)
        binary.write_bytes(header)
        libc = 'libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x123)'
        with patch('release.common.run', side_effect=[libc, 'GLIBC_2.17 GLIBC_2.35']):
            record = inspect_binary(binary, 'x86_64-unknown-linux-gnu')
        self.assertEqual(record, {'libraries': ['/lib/x86_64-linux-gnu/libc.so.6'], 'required_glibc': '2.35'})
        cpp = 'libstdc++.so.6 => /lib/x86_64-linux-gnu/libstdc++.so.6 (0x123)'
        for listing, symbols in ((cpp, 'GLIBC_2.35'), (libc, 'GLIBC_2.35 GLIBCXX_3.4.29'), (libc, 'GLIBC_2.35 CXXABI_1.3.13')):
            with self.subTest(listing=listing, symbols=symbols), \
                    patch('release.common.run', side_effect=[listing, symbols]), \
                    self.assertRaisesRegex(ValueError, 'C\\+\\+ runtime'):
                inspect_binary(binary, 'x86_64-unknown-linux-gnu')


if __name__ == '__main__':
    unittest.main()
