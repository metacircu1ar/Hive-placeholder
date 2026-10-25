#!/usr/bin/env python3
"""Exercise plain release binaries across control, hosted knowledge and MCP."""
import argparse
import json
import os
import select
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from check_release_install import json_command, running, wait_for
from release.acceptance import Cli, mock_login, require
from release.common import ROOT, digest, native_target
from release.compatibility_peers import catalog, declarations, materialize
from release.compatibility_recipe import recipe, previous_line
from release.compatibility_corpus import StateCorpus, KnowledgeCorpus, completed
from release.compatibility_skew import protocol, overlap, probe
from release import compatibility_transcripts as transcripts
from release.files import atomic_write_bytes, atomic_write_json, atomic_write_text, create_directory
from release.progress import entry, phase, finalizer, cleanup_all


def identity(binary):
    if binary.name != 'hive-daemon':
        return json_command([str(binary), 'version', '--json'], env=os.environ)
    value = subprocess.check_output([str(binary), '--version'], text=True, timeout=10).strip()
    words = value.split()
    require(len(words) >= 2 and words[0] == 'hive-daemon', 'Unexpected daemon identity')
    return {'product_version': words[1], 'display': value}


def image(root):
    return {str(p.relative_to(root)): (p.stat().st_ino, p.stat().st_mode, digest(p))
            for p in root.rglob('*') if p.is_file()}


def refuse_newer(root, marker, command, env):
    original = marker.read_bytes()
    atomic_write_json(marker, {'format': 4294967295})
    before = image(root)
    with finalizer(lambda: atomic_write_bytes(marker, original)):
        result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=15, check=False)
        require(result.returncode != 0, 'Older binary accepted newer data')
        require(json.loads(result.stdout)['error']['code'] == 'newer_format', 'Wrong newer-data refusal')
        require(image(root) == before, 'Refusal changed owned data')


def check_previous_reader(root, marker, binary, family, command, env):
    actual = json.loads(marker.read_bytes())['format']
    supported = next(item for item in identity(binary)['formats'] if item['name'] == family)
    before = image(root)
    result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=15, check=False)
    if actual > supported['maximum']:
        require(result.returncode != 0 and json.loads(result.stdout)['error']['code'] == 'newer_format', 'Previous reader failed newer-format refusal')
    else:
        from release.process import json_reply
        require(json_reply(result)['status'] == 'current', 'Previous reader cannot inspect advertised compatible data')
    require(image(root) == before, 'Previous reader changed candidate data')


def mcp(command, tool, arguments, env, *, offer='2025-11-25', selected='2025-11-25'):
    requests = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': offer, 'capabilities': {}, 'clientInfo': {'name': 'compat', 'version': 'synthetic-0'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {'name': tool, 'arguments': arguments}},
    ]
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
    def close():
        def stop():
            if process.poll() is None:
                process.kill()
            process.wait(timeout=5)
        cleanup_all(stop, process.stdin.close, process.stdout.close)
    replies, buffer = [], bytearray()
    with finalizer(close):
        process.stdin.write(''.join(json.dumps(r) + '\n' for r in requests).encode())
        process.stdin.flush()
        deadline = time.monotonic() + 30
        while len(replies) < 2:
            left = deadline - time.monotonic()
            require(left > 0, 'MCP transcript timed out')
            ready, _, _ = select.select([process.stdout], [], [], left)
            require(bool(ready), 'MCP transcript timed out')
            data = os.read(process.stdout.fileno(), 8192)
            require(bool(data), 'MCP closed before replying')
            buffer.extend(data)
            require(len(buffer) <= 1024 * 1024, 'MCP transcript exceeded its bound')
            while b'\n' in buffer:
                line, _, rest = buffer.partition(b'\n')
                buffer = bytearray(rest)
                replies.append(json.loads(line))
        process.stdin.close()
        require(process.wait(timeout=5) == 0, 'MCP process failed')
    require(len(replies) == 2 and replies[0]['id'] == 1 and replies[1]['id'] == 2, 'MCP transcript lost a reply')
    require(replies[0]['result']['protocolVersion'] == selected, 'MCP negotiation mismatch')
    result = replies[1]['result']
    require(not result.get('isError', False), 'MCP operation failed')
    return json.loads(result['content'][0]['text'])


class Matrix:
    def __init__(self, output, work, fixtures):
        self.output, self.work = output, work
        self.env = dict(os.environ)
        self.inputs = json.loads(fixtures['scenario'])
        self.transcripts = json.loads(fixtures['transcripts'])
        self.provider = work / 'mock-provider'
        create_directory(work)
        os.chmod(work, 0o700)
        atomic_write_bytes(self.provider, f'#!{sys.executable}\n'.encode() + fixtures['provider'], mode=0o700)
        for key, name in [('provider_schema', 'codex-schema.json'), ('provider_common', 'mock_common.py'),
                          ('publication', 'mock_files.py'), ('embeddings', 'embeddings.py')]:
            atomic_write_bytes(work / name, fixtures[key], mode=0o600)

    def pair(self, label, cli, daemon, comb_client, comb_server, current):
        work = self.work / label
        create_directory(work)
        os.chmod(work, 0o700)
        model_log = self.output / (label + '-embeddings.log')
        with running([sys.executable, str(self.work / 'embeddings.py')], model_log, self.env) as model:
            port = wait_for(lambda: model_log.read_text().strip(), 'mock embedding startup', model)
            config = work / 'embedding.json'
            atomic_write_json(config, dict(self.inputs['embedding'], url=f'http://127.0.0.1:{port}/v1/embeddings'))
            return self._pair(label, work, config, cli, daemon, comb_client, comb_server, current)

    def _pair(self, label, work, embedding, cli, daemon, comb_client, comb_server, current):
        knowledge, socket = work / 'knowledge', work / 'comb.sock'
        producer = [str(comb_server), '--root', str(knowledge), '--socket', str(socket)]
        consumer = [str(comb_client), '--root', str(knowledge), '--socket', str(socket)]
        invoke = lambda *args: json_command(consumer + list(args), env=self.env)
        json_command([str(comb_server), '--root', str(knowledge), 'init'], env=self.env)
        brain = KnowledgeCorpus(work, self.inputs)
        corpus = StateCorpus(work, self.inputs)
        with running(producer + ['serve', '--embedding-config', str(embedding)], self.output / (label + '-comb.log'), self.env) as server:
            wait_for(lambda: socket.exists() and invoke('stats'), 'compat Comb startup', server)
            bridge = work / 'bridge.json'
            # The server and Hive daemon are opposite generations in both skew
            # directions. Its CLI writes the connection that Hive will consume.
            json_command(producer + ['credential', 'create', 'compat-bridge', '--bridge', '--out', str(bridge)], env=self.env)
            reader = work / 'reader.json'
            json_command(producer + ['credential', 'create', 'compat-reader', '--out', str(reader)], env=self.env)
            if comb_client == comb_server:
                transcripts.service(socket, reader, self.transcripts['service'])
            state = work / 'state'
            config = work / 'config.json'
            instructions = work / 'role.md'
            atomic_write_text(instructions, 'Deterministic compatibility fixture.\n')
            atomic_write_json(config, {
                'auth_profiles': {'mock': {'provider': 'codex', 'method': 'subscription'}},
                'roles': {'implementor': {'auth': 'mock', 'model': self.inputs['model'], 'effort': self.inputs['effort'],
                                         'instructions': str(instructions), 'lifetime': 'independent', 'sandbox': 'read-only'}},
                'comb': {'credential_file': str(bridge)},
                'comb_collectors': {'shared': {'credential_file': str(bridge), 'sources': ['correction']}},
            }, mode=0o600)
            args = ['--state-dir', str(state), '--config', str(config), '--codex-bin', str(self.provider)]
            mock_login([str(daemon), *args], env=self.env)
            call = self.control(cli, state, config)
            with running([str(daemon), *args, 'daemon', 'run'], self.output / (label + '-hive.log'), self.env) as process:
                wait = lambda probe, name: wait_for(probe, name, process)
                wait(lambda: (state / 'runtime/control.sock').exists() and call('daemon', 'status'), 'compat Hive startup')
                if cli == daemon:
                    transcripts.control(state / 'runtime/control.sock', self.transcripts['control'])
                def untrusted(page):
                    actions = [{'name': 'comb_put', 'arguments': {'slug': page['slug'], 'markdown': page['markdown']}}]
                    agent = call('spawn', 'implementor', '--cwd', str(work), '--body', 'comb:' + json.dumps(actions))['agent_id']
                    wait(lambda: completed(call, agent), 'compat hosted turn')
                brain.seed(invoke, untrusted)
                corpus.seed(call, wait)
                reply = mcp([str(cli), '--state-dir', str(state), 'mcp'], 'hive_call',
                            {'target': corpus.source, 'body': 'reply:compat-mcp', 'request_id': 'compat-mcp', 'timeout': 10}, self.env)
                require(reply['outcome'] == 'replied' and 'compat-mcp' in json.dumps(reply['result']), 'Control MCP reply mismatch')
                # Deny the collector its credential only after all hosted operations
                # are complete, so this row is definitely pending at shutdown.
                hidden = work / 'bridge-hidden.json'
                bridge.rename(hidden)
                evidence_args = ('evidence', 'correction', '--summary', self.inputs['collector'], '--request-id', 'compat-evidence')
                evidence = call(*evidence_args)
                require(call('collectors', 'status')['collectors'][0]['pending'] == 1, 'Outbox corpus was not pending')
                call('daemon', 'stop')
                require(process.wait(timeout=20) == 0, 'Compatibility daemon failed to stop')
                hidden.rename(bridge)
            before_search = wait_for(lambda: brain.searches(invoke), 'corpus embeddings', server)
            cases = self.transcripts['mcp'] if comb_client == comb_server else [self.transcripts['mcp'][0]]
            for case in cases:
                page = mcp([str(comb_client), 'mcp', '--credential', str(reader)], 'comb_get',
                           {'slug': self.inputs['pages']['trusted']['slug']}, self.env, offer=case['offer'], selected=case['selected'])
                require(page == brain.documents[self.inputs['pages']['trusted']['slug']], 'Knowledge MCP read mismatch')
        # Both roots are quiescent. All data is produced by the selected binaries;
        # only current binaries reopen it after the upgrade.
        for command in ([str(current / 'hivectl'), '--state-dir', str(state), 'state', 'upgrade'],
                        [str(current / 'comb'), '--root', str(knowledge), 'upgrade']):
            json_command(command + ['--dry-run'], env=self.env)
            json_command(command, env=self.env)
        upgraded = [str(current / 'comb'), '--root', str(knowledge), '--socket', str(socket)]
        invoke_current = lambda *args: json_command(upgraded + list(args), env=self.env)
        with running(upgraded + ['serve', '--embedding-config', str(embedding)], self.output / (label + '-comb-upgraded.log'), self.env) as server:
            wait_for(lambda: socket.exists() and invoke_current('stats'), 'upgraded Comb startup', server)
            brain.verify(invoke_current, before_search)
            page = mcp([str(current / 'comb'), 'mcp', '--credential', str(reader)], 'comb_get',
                       {'slug': self.inputs['pages']['trusted']['slug']}, self.env)
            require(page == brain.documents[self.inputs['pages']['trusted']['slug']], 'Upgrade lost credential authorization')
            latest = self.control(current / 'hivectl', state, config)
            with running([str(current / 'hivectl'), *args, 'daemon', 'run'], self.output / (label + '-resumed.log'), self.env) as process:
                wait = lambda probe, name: wait_for(probe, name, process)
                wait(lambda: (state / 'runtime/control.sock').exists() and latest('status', corpus.child), 'upgraded agent visibility')
                corpus.verify(latest, wait, state)
                require(latest(*evidence_args) == evidence, 'Upgrade lost collector request deduplication')
                wait(lambda: latest('collectors', 'status')['collectors'][0]['pending'] == 0, 'upgraded outbox drain')
                pages = json_command(upgraded + ['list', '--prefix', 'inbox/'], env=self.env, allow_list=True)
                matching = [p for p in pages if self.inputs['collector'] in json.dumps(invoke_current('get', p['slug']))]
                require(len(matching) == 1, 'Upgraded outbox lost or duplicated capture')
                latest('daemon', 'stop')
                require(process.wait(timeout=20) == 0, 'Upgraded daemon failed to stop')
        check_previous_reader(state, state / 'format', daemon, 'state', [str(daemon), '--state-dir', str(state), 'state', 'upgrade', '--dry-run'], self.env)
        check_previous_reader(knowledge, knowledge / '.comb/format', comb_server, 'root', [str(comb_server), '--root', str(knowledge), 'upgrade', '--dry-run'], self.env)
        refuse_newer(state, state / 'format', [str(daemon), '--state-dir', str(state), 'state', 'upgrade', '--dry-run'], self.env)
        refuse_newer(knowledge, knowledge / '.comb/format', [str(comb_server), '--root', str(knowledge), 'upgrade', '--dry-run'], self.env)
        return {'control': True, 'hosted': True, 'collector': True, 'mcp': True, 'connection_files': True,
                'upgrade_resume': True, 'newer_data_refusal': True, 'branch': True, 'attachments': True,
                'artifacts': True, 'trust': True, 'receipts': True, 'credentials': True,
                'text': True, 'vector': True, 'hybrid': True}

    def outside_window(self, label, cli, daemon, comb_client, comb_server):
        work = self.work / label
        create_directory(work)
        os.chmod(work, 0o700)
        root, socket = work / 'knowledge', work / 'comb.sock'
        producer = [str(comb_server), '--root', str(root), '--socket', str(socket)]
        json_command([str(comb_server), '--root', str(root), 'init'], env=self.env)
        service_shared = overlap(protocol(identity(comb_client), 'service_protocol'), protocol(identity(comb_server), 'service_protocol'))
        with running(producer + ['serve'], self.output / (label + '-comb.log'), self.env) as server:
            wait_for(lambda: socket.exists() and json_command(producer + ['stats'], env=self.env), 'old-peer service startup', server)
            service = probe([str(comb_client), '--root', str(root), '--socket', str(socket), 'stats'], service_shared, 'Comb', self.env)
        config = work / 'config.json'
        atomic_write_json(config, {'auth_profiles': {'mock': {'provider': 'codex', 'method': 'subscription'}}, 'roles': {}})
        state = work / 'state'
        args = [str(daemon), '--state-dir', str(state), '--config', str(config), '--codex-bin', str(self.provider)]
        mock_login(args, env=self.env)
        control_shared = overlap(protocol(identity(cli), 'control_protocol'), protocol(identity(daemon), 'control_protocol'))
        with running(args + ['daemon', 'run'], self.output / (label + '-hive.log'), self.env) as process:
            native = self.control(daemon, state, config)
            wait_for(lambda: (state / 'runtime/control.sock').exists() and native('daemon', 'status'), 'old-peer daemon startup', process)
            control = probe([str(cli), '--state-dir', str(state), 'agents'], control_shared, 'Hive', self.env)
            native('daemon', 'stop')
            require(process.wait(timeout=20) == 0, 'Old-peer daemon failed to stop')
        return {'control': control, 'service': service}

    def control(self, binary, state, config):
        control = [str(binary), '--state-dir', str(state), '--config', str(config)]
        return Cli([str(binary), '--state-dir', str(state)], prefixes={'daemon': control, 'auth': control}, timeout=15)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--current-dir', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--peers', type=Path, default=ROOT / 'compat/releases.json')
    args = parser.parse_args()
    current = args.current_dir.resolve()
    require('release' in current.parts, 'Current binaries must come from a plain release build')
    require(not args.output_dir.exists(), 'Compatibility output must be new')
    create_directory(args.output_dir)
    os.chmod(args.output_dir, 0o700)
    rows = catalog(args.peers.read_bytes())
    target = native_target()
    declarations(ROOT, rows, target)
    peers = [row for row in rows if row['target'] == target]
    require(not rows or bool(peers), 'Declared releases lack checksum-pinned assets for this target')
    results = {'status': 'incomplete', 'current': {}, 'synthetic': None, 'released': [], 'historical_status': 'not_declared' if not rows else 'pending'}
    for name in ('hivectl', 'hive-daemon', 'comb'):
        results['current'][name] = {'sha256': digest(current / name), 'identity': identity(current / name)}
    with finalizer(lambda: atomic_write_json(args.output_dir / 'results.json', results, mode=0o600)), tempfile.TemporaryDirectory(prefix='lx-compat-', dir='/tmp') as directory:
        matrix = Matrix(args.output_dir, Path(directory) / 'synthetic', recipe(ROOT, None, synthetic=True))
        with phase('verify'):
            results['synthetic'] = matrix.pair('synthetic-0', current / 'hivectl', current / 'hivectl', current / 'comb', current / 'comb', current)
        skew = previous_line([row['version'] for row in peers], results['current']['hivectl']['identity']['product_version'])
        for row in peers:
            destination = Path(directory) / row['version'] / 'release'
            with phase('fetch'):
                materialize(row, destination)
            for name in ('hivectl', 'hive-daemon', 'comb'):
                require(identity(destination / name)['product_version'] == row['version'], 'Downloaded peer version mismatch')
            historical = Matrix(args.output_dir, Path(directory) / (row['version'] + '-corpus'),
                                recipe(ROOT, row['version'], expected_sha256=row['corpus_sha256']))
            with phase('verify'):
                upgraded = historical.pair(row['version'] + '-upgrade', destination / 'hivectl', destination / 'hivectl', destination / 'comb', destination / 'comb', current)
                if row['version'] not in skew:
                    # All historical corpora still upgrade; unsupported protocol
                    # directions are checked separately, never required to work.
                    old_new = historical.outside_window(row['version'] + '-old-protocol', destination / 'hivectl', current / 'hivectl', current / 'comb', destination / 'comb')
                    new_old = historical.outside_window(row['version'] + '-new-protocol', current / 'hivectl', destination / 'hivectl', destination / 'comb', current / 'comb')
                    results['released'].append({'version': row['version'], 'upgrade': upgraded, 'skew': 'outside_window', 'old_cli_new_daemon': old_new, 'new_cli_old_daemon': new_old})
                    continue
                old_new = historical.pair(row['version'] + '-old-cli', destination / 'hivectl', current / 'hivectl', current / 'comb', destination / 'comb', current)
                new_old = historical.pair(row['version'] + '-new-cli', current / 'hivectl', destination / 'hivectl', destination / 'comb', current / 'comb', current)
            results['released'].append({'version': row['version'], 'upgrade': upgraded, 'old_cli_new_daemon': old_new, 'new_cli_old_daemon': new_old})
        results['historical_status'] = 'executed' if rows else 'not_declared'
        results['status'] = 'passed'
    print(json.dumps({'synthetic': 'passed', 'released_pairs': len(peers), 'historical_status': results['historical_status']}))
    return 0


if __name__ == '__main__':
    raise SystemExit(entry(main, name='verify'))
