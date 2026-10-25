#!/usr/bin/env python3
"""Build and immediately freeze one plain package-scoped release variant."""
import argparse
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from release.common import PARTS, ROOT, digest, inspect_binary, native_target, run, source_identity, write_json
from release.licenses import collect
from release.progress import entry, phase
from release.files import atomic_copy, atomic_write_text, sync_directory, create_directory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--part', choices=PARTS, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--clean', action='store_true', help='Use a new disposable target directory for a clean-build measurement')
    args = parser.parse_args()
    target = native_target()
    if args.output_dir.exists():
        parser.error('Output directory already exists; choose a fresh freeze directory')
    package, flags, binaries, variant = PARTS[args.part]
    before = source_identity()
    with tempfile.TemporaryDirectory(prefix='hive-release-') as scratch:
        env = dict(os.environ, CARGO_BUILD_JOBS='4')
        if args.clean:
            env['CARGO_TARGET_DIR'] = scratch
        command = ['cargo', 'build', '--release', '--locked', '-p', package, *flags, '--message-format=json-render-diagnostics']
        began = time.monotonic()
        compiled = subprocess.run(command, cwd=ROOT, env=env, check=True, stdout=subprocess.PIPE, text=True)
        artifacts = {}
        for line in compiled.stdout.splitlines():
            frame = json.loads(line)
            if frame.get('reason') == 'compiler-artifact' and frame.get('executable') and frame['target']['name'] in binaries:
                if frame['profile']['test'] or frame['profile']['opt_level'] != '3' or frame['profile']['debug_assertions']:
                    raise ValueError('Shipping binaries must use the normal optimized release profile')
                artifacts[frame['target']['name']] = frame
        if set(artifacts) != set(binaries):
            raise ValueError('Cargo did not identify every requested executable')
        elapsed = time.monotonic() - began
        if source_identity() != before:
            raise ValueError('Source changed during build; refusing to attribute this binary')
        create_directory(args.output_dir)
        out = args.output_dir / 'release'; out.mkdir()
        records = {}
        for name in binaries:
            path = out / name
            atomic_copy(artifacts[name]['executable'], path, mode=0o755, create_once=True)
            version = run([str(path.resolve()), '--version'])
            fields = version.split()
            if len(fields) < 2:
                raise ValueError('Executable did not report a product version')
            product_version = fields[1]
            if variant and not version.endswith(f'({variant})'):
                raise ValueError(f'Unexpected compiled variant: {version}')
            records[name] = {'bytes': path.stat().st_size, 'sha256': digest(path), 'version': version, 'product_version': product_version, 'dependencies': inspect_binary(path, target), 'cargo_features': artifacts[name]['features'], 'cargo_profile': artifacts[name]['profile']}
        with phase('licenses'):
            dependencies, notices = collect(package, flags, target)
        atomic_write_text(args.output_dir / 'THIRD_PARTY_LICENSES', notices, mode=0o644, create_once=True)
        sync_directory(out)
        sync_directory(args.output_dir)
        sync_directory(args.output_dir.parent)
        write_json(args.output_dir / 'build.json', {
            'part': args.part, 'target': target, 'profile': 'release', 'command': command,
            'source': before, 'rustc': run(['rustc', '-Vv']), 'clean': args.clean,
            'build_seconds': elapsed, 'environment': {k:v for k,v in env.items() if k in {'RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS'} or k.startswith('CARGO_PROFILE_RELEASE_')},
            'binaries': records, 'dependencies': dependencies, 'notices_sha256': digest(args.output_dir / 'THIRD_PARTY_LICENSES'),
            'notices_bytes': (args.output_dir / 'THIRD_PARTY_LICENSES').stat().st_size,
        })
    print(args.output_dir / 'build.json')


if __name__ == '__main__':
    raise SystemExit(entry(main, name='build'))
