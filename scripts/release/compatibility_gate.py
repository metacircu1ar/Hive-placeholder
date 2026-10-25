"""Fingerprint owned contracts; released entries are immutable, drafts are not.

Registry JSON is checked against the Rust registries by each package's release
tests. Contract fixtures must also pass their named production tests. This gate
does not infer schema coverage from Rust source or declare a product release.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

from .progress import entry

MANIFEST = 'scripts/release/compatibility.json'


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate compatibility key: ' + key)
        result[key] = value
    return result


def reject_constant(value):
    raise ValueError('Non-JSON compatibility constant: ' + value)


def decode(data):
    return json.loads(data, object_pairs_hook=unique, parse_constant=reject_constant)


def fields(value, keys):
    if not isinstance(value, dict) or set(value) != set(keys):
        raise ValueError('Unexpected compatibility record fields')


def positive(value):
    if type(value) is not int or not 0 < value <= 4294967295:
        raise ValueError('Format numbers must be positive u32 values')


def path(root, name):
    if not isinstance(name, str):
        raise ValueError('Contract paths must be strings')
    relative = PurePosixPath(name)
    if not name or relative.is_absolute() or '..' in relative.parts:
        raise ValueError('Contract paths must stay in the product repository')
    result = root / relative
    if result.is_symlink() or not result.resolve().is_relative_to(root.resolve()):
        raise ValueError('Contract path escapes the product repository')
    return result


def registry(data):
    rows = decode(data)
    if not isinstance(rows, list):
        raise ValueError('Compatibility registry must be an array')
    result = {}
    for row in rows:
        fields(row, ('name', 'current', 'minimum', 'maximum', 'released'))
        name = row['name']
        if not isinstance(name, str) or not re.fullmatch('[a-z][a-z0-9_]*', name) or name in result:
            raise ValueError('Invalid or duplicate format name')
        for key in ('current', 'minimum', 'maximum'):
            positive(row[key])
        if not row['minimum'] <= row['current'] <= row['maximum']:
            raise ValueError('Current format is outside its supported range')
        if not isinstance(row['released'], list):
            raise ValueError('Released contracts must be an array')
        releases = set()
        numbers = {}
        for release in row['released']:
            fields(release, ('product_version', 'number', 'fingerprint'))
            positive(release['number'])
            version = release['product_version']
            digest = release['fingerprint']
            if not isinstance(version, str) or not version or version in releases:
                raise ValueError('Invalid or duplicate declared release')
            if not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest):
                raise ValueError('Invalid released fingerprint')
            if release['number'] in numbers and numbers[release['number']] != digest:
                raise ValueError('One released number cannot describe two formats')
            numbers[release['number']] = digest
            releases.add(version)
        result[name] = row
    return result


def fingerprint(root, artifacts):
    if not isinstance(artifacts, list) or not artifacts:
        raise ValueError('Every format needs contract artifacts')
    pieces = {}
    for artifact in artifacts:
        fields(artifact, ('id', 'path', 'encoding', 'test'))
        identity = artifact['id']
        if not isinstance(identity, str) or not identity or identity in pieces:
            raise ValueError('Invalid or duplicate contract artifact ID')
        if not isinstance(artifact['test'], str) or not artifact['test']:
            raise ValueError('Every artifact needs a production contract test')
        data = path(root, artifact['path']).read_bytes()
        if artifact['encoding'] == 'json':
            data = json.dumps(decode(data), sort_keys=True, ensure_ascii=False,
                              allow_nan=False, separators=(',', ':')).encode()
        elif artifact['encoding'] != 'bytes':
            raise ValueError('Unknown contract fingerprint encoding')
        pieces[identity] = data
    digest = hashlib.sha256()
    for identity, data in sorted(pieces.items()):
        for piece in (identity.encode(), data):
            digest.update(len(piece).to_bytes(8, 'big'))
            digest.update(piece)
    return digest.hexdigest()


def preserve_releases(current, previous):
    for name, old in previous.items():
        if not old['released']:
            continue
        if name not in current:
            raise ValueError('A released format was removed: ' + name)
        releases = {r['product_version']: r for r in current[name]['released']}
        for release in old['released']:
            if releases.get(release['product_version']) != release:
                raise ValueError('A released fingerprint was removed or rewritten: ' + name)


def baseline_file(root, ref, name):
    # Check existence separately: a failed Git command is never missing history.
    exists = subprocess.check_output(['git', 'ls-tree', ref, '--', name], cwd=root)
    if not exists:
        return None
    return subprocess.check_output(['git', 'show', f'{ref}:{name}'], cwd=root)


def baseline_commit(root, ref):
    # Resolve a single revision before using it with other Git subcommands. No
    # caller-supplied option, revision range or path expression reaches ls-tree.
    return subprocess.check_output(
        ['git', 'rev-parse', '--verify', '--end-of-options', ref + '^{commit}'],
        cwd=root, text=True).strip()


def preserve_corpora(root, baseline):
    old = baseline_file(root, baseline, 'compat/releases.json')
    if old is None:
        return
    current = decode((root / 'compat/releases.json').read_bytes())
    pins = {(r['version'], r['target']): r for r in current}
    for row in decode(old):
        if pins.get((row['version'], row['target'])) != row:
            raise ValueError('A released binary or corpus pin was removed or rewritten')


def check(root, baseline=None):
    document = decode((root / MANIFEST).read_bytes())
    fields(document, ('registries', 'contracts'))
    if not isinstance(document['registries'], list) or not isinstance(document['contracts'], list):
        raise ValueError('Compatibility inventory lists required')
    registries = {}
    for item in document['registries']:
        fields(item, ('crate', 'path', 'test'))
        crate = item['crate']
        if not isinstance(crate, str) or not crate or crate in registries or not item['test']:
            raise ValueError('Duplicate or invalid crate registry')
        registries[crate] = registry(path(root, item['path']).read_bytes())
    if baseline:
        baseline = baseline_commit(root, baseline)
        preserve_corpora(root, baseline)
        old_inventory = baseline_file(root, baseline, MANIFEST)
        if old_inventory is not None:
            old_document = decode(old_inventory)
            for item in old_document['registries']:
                old = baseline_file(root, baseline, item['path'])
                if old is None:
                    raise ValueError('Baseline inventory references a missing registry')
                preserve_releases(registries.get(item['crate'], {}), registry(old))
    expected = {(crate, name) for crate, rows in registries.items() for name in rows}
    results = {}
    for contract in document['contracts']:
        fields(contract, ('crate', 'name', 'artifacts', 'remaining_coverage'))
        key = (contract['crate'], contract['name'])
        if key not in expected or key in results:
            raise ValueError('Duplicate or orphan contract inventory entry')
        remaining = contract['remaining_coverage']
        if not isinstance(remaining, list) or any(not isinstance(r, str) or not r for r in remaining):
            raise ValueError('Invalid contract coverage evidence')
        digest = fingerprint(root, contract['artifacts'])
        row = registries[key[0]][key[1]]
        if remaining:
            print(f'WARNING: incomplete compatibility coverage for {key[0]}/{key[1]}:', file=sys.stderr)
            for gap in remaining:
                print('  - ' + gap, file=sys.stderr)
        for release in row['released']:
            if release['number'] == row['current'] and release['fingerprint'] != digest:
                raise ValueError('Released contract changed: bump the number and add a migration step')
        results[key] = digest
    if set(results) != expected:
        raise ValueError('A registry entry has no contract inventory')
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--baseline', help='Immutable Git revision whose released entries must be preserved')
    args = parser.parse_args()
    checked = check(args.root, args.baseline)
    print(f'Checked {len(checked)} compatibility contracts; fixture tests are required separately')


if __name__ == '__main__':
    raise SystemExit(entry(main, name='verify'))
