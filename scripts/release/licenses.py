"""Retain notices from the exact normal dependency graph of a plain build."""
import json
import re
from pathlib import Path
from .common import ROOT, digest, run

ALLOWED = {"MIT", "MIT-0", "Apache-2.0", "LLVM-exception", "Unicode-3.0", "Zlib", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Unlicense", "0BSD", "BSL-1.0", "MPL-2.0", "CC0-1.0", "CDLA-Permissive-2.0"}
FIRST_PARTY = {
    'hive': 'Cargo.toml', 'comb': 'comb/Cargo.toml',
    'comb-protocol': 'comb-protocol/Cargo.toml',
    'hive-install': 'installer/Cargo.toml', 'hive-setup-support': 'setup-support/Cargo.toml',
    'hive-log': 'logging/Cargo.toml',
    'hive-input': 'input/Cargo.toml',
}


def first_party(entry):
    """A path dependency, even a workspace member, is not automatically our code."""
    relative = FIRST_PARTY.get(entry['name'])
    return (entry['source'] is None and relative is not None
            and Path(entry['manifest_path']).resolve() == (ROOT / relative).resolve())


def source_description(entry):
    source = entry['source']
    if source == 'registry+https://github.com/rust-lang/crates.io-index':
        return f"https://crates.io/api/v1/crates/{entry['name']}/{entry['version']}/download"
    return source or 'path dependency from the build checkout'


def collect(package, flags, target):
    graph = run(["cargo", "tree", "--locked", "-p", package, *flags, "--target", target, "--edges", "normal", "--prefix", "none", "--format", "{p}"])
    selected = set(re.findall(r"^([\w-]+) v([^\s]+)", graph, re.M))
    metadata = json.loads(run(["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target, *flags]))
    available = {(p['name'], p['version']) for p in metadata['packages']}
    if selected - available:
        raise ValueError(f"Dependency metadata missing: {selected - available}")
    records, text = [], []
    for entry in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if (entry["name"], entry["version"]) not in selected or first_party(entry):
            continue
        license = entry.get("license") or ""
        tokens = set(re.findall(r"[A-Za-z0-9][A-Za-z0-9.+-]*", license)) - {"AND", "OR", "WITH"}
        if not tokens or not tokens <= ALLOWED:
            raise ValueError(f"Unreviewed dependency license: {entry['name']} {license}")
        directory = Path(entry["manifest_path"]).parent
        files = set()
        for path in directory.rglob("*"):
            if any(word in path.name.lower() for word in ("license", "licence", "copying", "copyright", "notice")):
                files.update(p for p in (path.rglob('*') if path.is_dir() else [path]) if p.is_file())
        if entry.get("license_file"):
            files.add(directory / entry["license_file"])
        if not files:
            raise ValueError(f"Missing license text: {entry['name']} {entry['version']}")
        notices = []
        text.append(f"\n{'=' * 72}\n{entry['name']} {entry['version']} — {license}\n{entry.get('repository') or source_description(entry)}\nSource: {source_description(entry)}\n")
        for path in sorted(files):
            content = path.read_text()
            notices.append({"file": str(path.relative_to(directory)), "sha256": digest(path)})
            text.append(f"\n--- {path.relative_to(directory)} ---\n{content}\n")
        if entry['name'] == 'libsqlite3-sys':
            # SQLite itself is public domain; its distributed source carries the declaration.
            source = directory / 'sqlite3/sqlite3.c'
            sqlite = source.read_text()
            at = sqlite.index('The author disclaims copyright')
            declaration = sqlite[sqlite.rfind('/*', 0, at):sqlite.index('*/', at) + 2] + '\n'
            text.append('\n--- Bundled SQLite declaration ---\n' + declaration)
        records.append({"name": entry["name"], "version": entry["version"], "license": license, "notices": notices})
    rust_docs = Path(run(['rustc', '--print', 'sysroot'])) / 'share/doc/rust'
    std_notices = []
    for name in ['COPYRIGHT-library.html', 'licenses/MIT.txt', 'licenses/Apache-2.0.txt']:
        path = rust_docs / name
        if not path.is_file():
            raise ValueError(f'Missing toolchain redistribution notice: {path}')
        text.append(f"\n--- Rust standard library: {name} ---\n{path.read_text()}\n")
        std_notices.append({'file': name, 'sha256': digest(path)})
    records.append({'name': 'rust-standard-library', 'version': run(['rustc', '-V']),
                    'license': 'MIT OR Apache-2.0; see bundled exceptions', 'notices': std_notices})
    return records, ''.join(text)
