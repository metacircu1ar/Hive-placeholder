#!/usr/bin/env python3
"""Assemble a native part from a verified build_release.py freeze; never build here."""
import argparse
import json
from pathlib import Path
from release.common import PARTS, ROOT, digest, inspect_binary, native_target, write_json
from release.docs import product_files
from release import files as publication
from release.archive import Payload, json_bytes, write_archive
from release.progress import entry


def package(build_dir, output):
    build_bytes = (build_dir / 'build.json').read_bytes()
    build = json.loads(build_bytes)
    part = build['part']; target = build['target']
    package, flags, names, _ = PARTS[part]
    expected = ['cargo', 'build', '--release', '--locked', '-p', package, *flags, '--message-format=json-render-diagnostics']
    if build['profile'] != 'release' or build['command'] != expected or target != native_target():
        raise ValueError('Require a native plain package-scoped release build record')
    if set(build['binaries']) != set(names):
        raise ValueError('Build record has the wrong binary set')
    versions = [record['product_version'] for record in build['binaries'].values()]
    if any(not isinstance(v, str) or not v or len(v) > 128 for v in versions) or len(set(versions)) != 1:
        raise ValueError('Build record has inconsistent product versions')
    files, glibc = {}, []
    for name in names:
        path = build_dir / 'release' / name
        record = build['binaries'][name]
        dependencies = inspect_binary(path, target)
        if digest(path) != record['sha256'] or path.stat().st_size != record['bytes'] or dependencies != record['dependencies']:
            raise ValueError(f'Frozen build identity mismatch: {name}')
        if dependencies['required_glibc']:
            glibc.append(dependencies['required_glibc'])
        files['bin/' + name] = Payload(path, 0o755, record)
    required = max(glibc, key=lambda v: tuple(map(int, v.split('.')))) if glibc else None
    if required and tuple(map(int, required.split('.'))) > (2, 35):
        raise ValueError(f'Build requires GLIBC {required}; release floor is 2.35. Build on Ubuntu 22.04.')
    notice = build_dir / 'THIRD_PARTY_LICENSES'
    notice_record = {'sha256': build['notices_sha256'], 'bytes': build['notices_bytes']}
    if digest(notice) != notice_record['sha256'] or notice.stat().st_size != notice_record['bytes']:
        raise ValueError('Build notice identity mismatch')
    publication.create_directory(output, exist_ok=True)
    with publication.directory_lock(output):
        name = publish(build_dir, output, build, build_bytes, files, notice_record, required)
        publication.atomic_copy(ROOT / 'packaging/install.sh', output / 'install.sh', mode=0o755)
        checksums(output)
    print(output / name)


def publish(build_dir, output, build, build_bytes, files, notice_record, required):
    part, target = build['part'], build['target']
    if part == 'hive-install':
        name = f'hive-install-{target}'
        publication.atomic_copy(build_dir / 'release/hive-install', output / name,
                                expected=build['binaries']['hive-install'], mode=0o755)
        publication.atomic_copy(build_dir / 'THIRD_PARTY_LICENSES', output / (name + '.LICENSES'),
                                expected=notice_record, mode=0o644)
        publication.atomic_write_bytes(output / (name + '.build.json'), build_bytes, mode=0o644)
        return name
    component = 'hive' if part.startswith('hive') else 'comb'
    share = f'share/{component}/'
    files[share + 'THIRD_PARTY_LICENSES'] = Payload(build_dir / 'THIRD_PARTY_LICENSES', 0o644, notice_record)
    files[share + 'build.json'] = Payload(build_bytes, 0o644)
    for path, data in product_files().items():
        files[share + path] = Payload(data, 0o644)
    manifest = {'format': 1, 'product_version': next(iter(build['binaries'].values()))['product_version'],
                'part': part, 'target': target, 'required_glibc': required,
                'files': {name: payload.manifest() for name, payload in files.items()}}
    files['manifest.json'] = Payload(json_bytes(manifest), 0o644)
    name = f'{part}-{target}.tar.gz'
    publication.atomic_stream(output / name, lambda stream: write_archive(stream, files), mode=0o644)
    return name


def checksums(output):
    artifacts = sorted(p for p in output.iterdir() if
                       p.name.endswith('.tar.gz') or p.name.startswith('hive-install-') or p.name == 'install.sh')
    if any(p.is_symlink() or not p.is_file() for p in artifacts):
        raise ValueError('Release assets must be regular files, not symlinks')
    sums = ''.join(f'{digest(path)}  {path.name}\n' for path in artifacts)
    publication.atomic_write_text(output / 'SHA256SUMS', sums, mode=0o644)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-dir', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    package(args.build_dir, args.output_dir)


if __name__ == '__main__':
    raise SystemExit(entry(main, name='package'))
