"""Explicit product documentation and portable archive link closure."""
import posixpath
import re
from urllib.parse import unquote, urlsplit
from .common import ROOT

GUIDES = (
    'README.md', 'docs/cli-reference.md', 'docs/setup.md', 'docs/terminal.md',
    'docs/platforms-and-installation.md', 'docs/compatibility.md',
    'comb/README.md',
    'skills/hive/SKILL.md', 'skills/comb-curator/SKILL.md',
)


def product_files(root=ROOT):
    # Never glob docs/: internal plans must not accidentally enter an archive.
    examples = sorted(str(p.relative_to(root)) for p in (root / 'examples').rglob('*')
                      if p.is_file() and p.suffix in {'.md', '.yaml'})
    names = [*GUIDES, *examples, 'examples/qwen3-embedding.json', 'logo.png', 'comb/logo.png']
    files = {name: (root / name).read_bytes() for name in names}
    validate_links(files)
    return files


def validate_links(files):
    for name, body in files.items():
        if not name.endswith('.md'):
            continue
        text = body.decode()
        if re.search(r'(?:https?://[^\s)]*parallax|(?:\.\./|/)parallax(?:/|\b))', text, re.I):
            raise ValueError(f'Internal workspace reference in product documentation: {name}')
        # Fenced snippets are examples, not rendered navigation.
        visible = re.sub(r'^\s*(`{3,}|~{3,})[^\n]*\n.*?^\s*\1\s*$', '', text, flags=re.M | re.S)
        links = re.findall(r'\]\(\s*(<[^>]+>|[^\s)]+)', visible)
        links += re.findall(r'(?:src|href)=["\']([^"\']+)', visible)
        links += re.findall(r'^\s*\[[^\]]+\]:\s*(\S+)', visible, flags=re.M)
        for link in links:
            parsed = urlsplit(link.strip('<>'))
            if parsed.scheme or parsed.netloc or not parsed.path:
                continue
            target = posixpath.normpath(posixpath.join(posixpath.dirname(name), unquote(parsed.path)))
            if target not in files and not any(p.startswith(target.rstrip('/') + '/') for p in files):
                raise ValueError(f'Unbundled documentation link: {name} -> {link}')
