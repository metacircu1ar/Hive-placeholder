"""Strict, hash-pinned corpus inputs shared by synthetic and released producers."""
import hashlib
import re

from .compatibility_gate import decode, fields, path

FIXTURES = ('provider', 'provider_schema', 'provider_common', 'publication', 'embeddings', 'transcripts', 'scenario')
CHECKS = ('native_bindings', 'branch', 'attachments', 'artifacts', 'outbox_once',
          'trust', 'receipts', 'credentials', 'text', 'vector', 'hybrid')


def recipe(root, version, *, expected_sha256=None, synthetic=False):
    name = 'synthetic-0' if synthetic else version
    location = 'compat/' + name + '/corpus.json'
    raw = path(root, location).read_bytes()
    if expected_sha256 is not None and hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise ValueError('Release corpus recipe checksum mismatch')
    value = decode(raw)
    fields(value, ('kind', 'product_version', 'fixtures', 'checks'))
    if value['kind'] != ('synthetic' if synthetic else 'released') or value['product_version'] != version:
        raise ValueError('Corpus producer identity mismatch')
    if value['checks'] != list(CHECKS):
        raise ValueError('Corpus must verify every preservation obligation')
    fields(value['fixtures'], FIXTURES)
    loaded = {}
    for key, pin in value['fixtures'].items():
        fields(pin, ('path', 'sha256'))
        if not isinstance(pin['sha256'], str) or not re.fullmatch('[0-9a-f]{64}', pin['sha256']):
            raise ValueError('Corpus fixture needs a SHA-256 pin')
        selected = path(root, pin['path'])
        if not synthetic and not selected.resolve().is_relative_to((root / 'compat' / version).resolve()):
            raise ValueError('Released corpus fixtures must be frozen with their recipe')
        content = selected.read_bytes()
        if hashlib.sha256(content).hexdigest() != pin['sha256']:
            raise ValueError('Corpus fixture checksum mismatch: ' + key)
        loaded[key] = content
    scenario(decode(loaded['scenario']))
    transcripts(decode(loaded['transcripts']))
    return loaded


def transcripts(value):
    fields(value, ('control', 'service', 'mcp'))
    for kind, keys in [('control', ('request', 'expect')),
                       ('service', ('protocol', 'operation', 'expect')),
                       ('mcp', ('offer', 'selected'))]:
        if not isinstance(value[kind], list) or not value[kind]:
            raise ValueError('Corpus requires control, service and MCP transcripts')
        for case in value[kind]:
            fields(case, keys)
            if kind != 'mcp' and (not isinstance(case['expect'], dict) or not case['expect']):
                raise ValueError('Transcript requires verification expectations')


def scenario(value):
    fields(value, ('model', 'effort', 'output', 'branch_body', 'attachment', 'queued',
                   'pages', 'summary', 'reason', 'capture', 'collector', 'queries', 'embedding'))
    fields(value['output'], ('text', 'repetitions'))
    if not isinstance(value['output']['text'], str) or not value['output']['text'] or type(value['output']['repetitions']) is not int or not 32768 <= len(value['output']['text']) * value['output']['repetitions'] <= 65500:
        raise ValueError('Corpus must exercise retained large output')
    for key in ('model', 'effort', 'branch_body', 'attachment', 'summary', 'reason', 'capture', 'collector'):
        if not isinstance(value[key], str) or not value[key].strip():
            raise ValueError('Corpus requires nonempty input: ' + key)
    for key in ('queued', 'queries'):
        if not isinstance(value[key], list) or len(value[key]) < 2 or any(not isinstance(v, str) or not v.strip() for v in value[key]) or len(set(value[key])) != len(value[key]):
            raise ValueError('Corpus requires distinct input cases: ' + key)
    fields(value['pages'], ('trusted', 'untrusted', 'note', 'proposal'))
    for key, page in value['pages'].items():
        fields(page, ('slug', 'markdown', 'unverified'))
        if not isinstance(page['slug'], str) or not isinstance(page['markdown'], str) or not page['markdown'].startswith('---\n') or page['unverified'] is not (key == 'untrusted'):
            raise ValueError('Corpus page expectation mismatch')
    fields(value['embedding'], ('backend', 'model', 'revision', 'dimensions', 'query_prefix', 'document_prefix'))
    if value['embedding']['backend'] != 'http' or value['embedding']['dimensions'] != 8:
        raise ValueError('Corpus requires deterministic eight-dimensional HTTP embeddings')
    return value


def previous_line(versions, current):
    """Patch releases share a line; only the previous minor/major line gets skew."""
    current_line = tuple(map(int, current.split('.')[:2]))
    lines = {tuple(map(int, value.split('.')[:2])) for value in versions}
    older = {line for line in lines if line < current_line}
    previous = max(older) if older else None
    return {value for value in versions if tuple(map(int, value.split('.')[:2])) in (current_line, previous)}
