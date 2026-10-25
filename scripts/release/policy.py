"""Product text and tracked-file boundaries; maintainer material belongs in AGENTS."""
import re
import subprocess
from pathlib import PurePosixPath

from .docs import GUIDES, product_files

# Exact product entrypoints; adding another guide requires classifying it here.
EXAMPLES = (
    'examples/auth.yaml', 'examples/comb.yaml', 'examples/comb-bootstrap.md',
    'examples/comb-curate.md', 'examples/qwen3-embedding.json',
)
ROOT_FILES = {
    'compat/synthetic-0/transcripts.json',
    'compat/synthetic-0/corpus.json',
    'compat/synthetic-0/scenario.json',
    'compat/releases.json',
    'scripts/release/compatibility.json',
    'tests/fixtures/codex-schema.json',
    'src/os/keychain.m',  # Native macOS identity-bound credential deletion.
    'tests/fixtures/keychain_query.m',  # Inert Security API query validation.
    '.gitignore', '.github/workflows/rust.yml', 'AGENTS.md', 'Cargo.toml', 'Cargo.lock', 'LICENSE',
    'build.rs', 'setup-support/build.rs', 'rust-toolchain.toml', 'clippy.toml', 'pyproject.toml',
}
ICONS = {f'icons/generated/{name}' for name in (
    'hive-64.data-uri', 'comb-64.data-uri',
)}
PRODUCT_PYTHON = {
    'tests/test_runtime_provider.py', 'tests/test_release_packaging.py',
    'tests/test_installer_arguments.py',
    'scripts/build_release.py', 'scripts/package_release.py', 'scripts/check_release_install.py',
    'scripts/check_compatibility.py',
}
MOCKS = {f'tests/fixtures/{name}.py' for name in (
    'claude_canary', 'claude_provider', 'claude_shell_gate', 'mock_common', 'mock_files', 'provider',
    'pty_attach', 'pty_login', 'pty_setup', 'runtime_provider', 'compatibility_embeddings',
)}
SOURCE_ROOTS = ('src/', 'comb/src/', 'comb-protocol/src/', 'installer/src/', 'setup-support/src/', 'logging/src/', 'input/src/')
PACKAGES = ('comb', 'comb-protocol', 'installer', 'setup-support', 'logging', 'input')


def allowed_path(name):
    """Classify tracked product paths; no generic docs/examples/scripts wildcard."""
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts:
        return False
    if name in ROOT_FILES | ICONS | {'logo.png', 'comb/logo.png'} | PRODUCT_PYTHON | MOCKS | set(GUIDES) | set(EXAMPLES):
        return True
    if name in {f'{package}/Cargo.toml' for package in PACKAGES}:
        return True
    if name.startswith(SOURCE_ROOTS):
        return path.suffix in {'.rs', '.c', '.h', '.sql', '.json'}
    if name.startswith('comb/tests/'):
        return path.suffix == '.rs' or name.startswith('comb/tests/fixtures/')
    if name.startswith(('tests/cli/', 'tests/runtime/', 'tests/support/', 'tests/setup_cli/')):
        return path.suffix == '.rs'
    if path.parent == PurePosixPath('tests') and path.suffix == '.rs':
        return True
    if name.startswith('scripts/release/'):
        if path.name in {prefix + gate + '.py' for prefix in ('', 'test_')
                         for gate in ('error_gate', 'write_gate', 'log_gate', 'python_error_gate')}:
            return False
        return path.suffix == '.py' and '__pycache__' not in path.parts
    return name == 'packaging/install.sh'


def check_tracked(root):
    output = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root)
    paths = [p.decode() for p in output.split(b'\0') if p]
    invalid = [p for p in paths if not allowed_path(p)]
    if invalid:
        raise ValueError('Unclassified tracked product files: ' + ', '.join(invalid))
    return paths


INTERNAL = re.compile(
    r'\b(?:parallax|scenarios?|benchmarks?|historical|research|unreleased)\b'
    r'|validation\s+legs?\b|target/release|cargo\s+(?:test|clippy|fmt)\b'
    r'|run_live_comb|scenario-runs|scripts/run_|\bdist/', re.I,
)
STAGE = re.compile(r'\b[BVRPDF][0-9]{1,2}\b')
DATE = re.compile(r'\b\d{4}-\d{2}-\d{2}\b')
FENCES = re.compile(r'^\s*(`{3,}|~{3,})[^\n]*\n.*?^\s*\1\s*$', re.M | re.S)
INLINE_CODE = re.compile(r'(`+).*?\1')


def check_text(name, text, *, markdown=False, literal=False):
    for rule in (INTERNAL, STAGE):
        match = rule.search(text)
        if match:
            raise ValueError(f'Non-product text in {name}: {match.group()}')
    # Literal examples may contain timestamps. Narrative dates are internal meta.
    prose = INLINE_CODE.sub('', FENCES.sub('', text)) if markdown else text
    match = None if literal else DATE.search(prose)
    if match:
        raise ValueError(f'Internal date in {name}: {match.group()}')


# CLI help comes from doc comments and attributes. MCP descriptions/instructions,
# setup/installer output and hosted bootstrap text come from these source owners.
TEXT_OWNERS = (
    'src/cli.rs', 'src/cli/**/*.rs', 'src/bin/*.rs',
    'src/external_mcp.rs', 'src/external_mcp/**/*.rs', 'src/runtime/api/tools.rs',
    'src/runtime/api/request.rs',
    'src/runtime/worker/session/startup.rs',
    'comb/src/cli.rs', 'comb/src/cli/**/*.rs', 'comb/src/provision/**/*.rs',
    'comb/src/setup.rs', 'comb/src/setup/**/*.rs',
    'comb-protocol/src/service/input/retrieval/metadata.rs',
    'comb-protocol/src/service/input/mutation/metadata.rs',
    'comb-protocol/src/service/input/admin.rs',
    'comb/src/service/options.rs',
    'comb-protocol/src/service/input/admin/metadata.rs',
    'comb-protocol/src/service/mcp/**/*.rs', 'comb-protocol/src/values/prefix.rs',
    'installer/src/**/*.rs', 'setup-support/src/**/*.rs',
)
# This source path is executable logic, not emitted help. Do not exempt a file.
NON_DISPLAY_LITERALS = {
    ('src/cli/setup/parts.rs', 'target/release'),
}
# Tokenization skips implementation comments, while retaining clap doc comments.
RUST_TEXT = re.compile(
    r'(?P<comment>//[^\n]*|/\*.*?\*/)'
    r'|(?:br|r)(?P<hashes>\#*)"(?P<raw>.*?)"(?P=hashes)'
    r'|b?"(?P<string>(?:\\.|[^"\\])*)"'
    r"|'(?:\\.|[^'\\])'", re.S,
)


def rust_text(source):
    for match in RUST_TEXT.finditer(source):
        if match.group('comment') is not None:
            if match.group().startswith('///') and not match.group().startswith('////'):
                yield match.group()[3:], False
            elif match.group().startswith('/**') and not match.group().startswith('/***'):
                yield match.group()[3:-2], False
        elif match.group('raw') is not None:
            yield match.group('raw'), True
        elif match.group('string') is not None:
            # Rust joins backslash-newline continuations before presenting help.
            yield re.sub(r'\\\n\s*', '', match.group('string')), True


def check_product_text(root):
    # Product docs share the archive's exact closure. Every example is linted too.
    files = product_files(root)
    for name, data in files.items():
        if name.endswith(('.md', '.yaml', '.json')):
            check_text(name, data.decode(), markdown=name.endswith('.md'),
                       literal=name.endswith(('.yaml', '.json')))
    for name in ['Cargo.toml', *(f'{p}/Cargo.toml' for p in PACKAGES)]:
        for description in re.findall(r'^description\s*=\s*"(.*)"', (root / name).read_text(), re.M):
            check_text(name, description)
    paths = sorted({p for pattern in TEXT_OWNERS for p in root.glob(pattern)})
    for path in paths:
        name = path.relative_to(root).as_posix()
        if 'tests' in path.relative_to(root).parts or path.stem.endswith('_tests'):
            continue
        for text, literal in rust_text(path.read_text()):
            if (name, text) not in NON_DISPLAY_LITERALS:
                # Native protocol dates are exact machine values, not prose.
                check_text(name, text, literal=literal and bool(DATE.fullmatch(text)))
    # Bootstrap shell help and diagnostics are shipped too, not only Rust help.
    check_text('packaging/install.sh', (root / 'packaging/install.sh').read_text(), literal=True)
