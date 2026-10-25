"""Probe out-of-window peers: an unsupported protocol must refuse explicitly."""
import json
import subprocess

from .acceptance import require
from .process import json_reply


def protocol(identity, name):
    rows = list(identity['formats'])
    for entries in identity.get('shared_formats', {}).values():
        rows.extend(entries)
    return next(row for row in rows if row['name'] == name)


def overlap(left, right):
    return max(left['minimum'], right['minimum']) <= min(left['maximum'], right['maximum'])


def outcome(result, shared, part):
    if shared:
        json_reply(result)
        return 'shared_protocol'
    require(result.returncode != 0, 'Out-of-window protocol was silently accepted')
    error = json.loads(result.stdout)['error']
    require(error['code'] == 'incompatible_protocol' and part.lower() in error['message'].lower(),
            'Out-of-window refusal must name the part to upgrade')
    return 'incompatible_protocol'


def probe(command, shared, part, env):
    return outcome(subprocess.run(command, capture_output=True, text=True, env=env, timeout=15, check=False), shared, part)
