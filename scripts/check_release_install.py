#!/usr/bin/env python3
"""Native installed-release acceptance: deterministic agents and embedding endpoint.

Run after build_release.py/package_release.py assemble all four native parts.
No real provider credentials, external network calls, real models or Cargo test binaries.
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from release.acceptance import Cli, event_pages, mock_login, require, verified_bootstrap
from release.files import atomic_write_text, atomic_copy, create_directory, raw_log
from release.common import PARTS, ROOT, digest, inspect_binary, native_target, write_json
from release.progress import entry, phase, finalizer
from release.process import json_reply


def wait_for(probe, label, process=None, seconds=45):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        if process is not None:
            require(process.poll() is None, f'{label}: process exited with {process.returncode}')
        try:
            result = probe()
            if result:
                return result
        except (RuntimeError, json.JSONDecodeError) as error:
            last = error
        time.sleep(0.05)
    raise RuntimeError(f'{label} timed out: {last}')


@contextmanager
def running(command, log_path, env):
    with raw_log(log_path) as log:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env)
    def stop():
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
                raise RuntimeError(f'Owned process failed to stop; inspect {log_path}')

        require(process.returncode == 0, f'Owned process exited with {process.returncode}; inspect {log_path}')
    with finalizer(stop):
        yield process


def json_command(command, *, env, body=None, allow_list=False):
    result = subprocess.run(command, input=body, capture_output=True, text=True, env=env, timeout=30, check=False)
    return json_reply(result, allow_list=allow_list)


@contextmanager
def embedding_server():
    """OpenAI-style embedding endpoint with constant vectors, never a real model."""
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            body = json.dumps({'data': [{'index': index, 'embedding': [1.0, 0.0]}
                                        for index in range(len(request['input']))]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body))); self.end_headers()
            self.wfile.write(body)
        def log_message(self, *_):
            pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever); thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/v1/embeddings'
    finally:
        server.shutdown(); thread.join(); server.server_close()


class InstalledChecks:
    def __init__(self, assets, output, work):
        self.assets, self.output, self.work = assets, output, work
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('DYLD_') and k != 'LD_LIBRARY_PATH'}
        self.bootstrap = verified_bootstrap(assets, work)
        self.provider = work / 'mock-provider'
        atomic_write_text(self.provider,
            f'#!{sys.executable}\nimport sys\n'
            'if "--version" in sys.argv:\n    print("codex-cli 0.161.0"); sys.exit(0)\n'
            + (ROOT / 'tests/fixtures/runtime_provider.py').read_text(), mode=0o700
        )
        atomic_copy(ROOT / 'tests/fixtures/codex-schema.json', work / 'codex-schema.json', mode=0o600)
        atomic_copy(ROOT / 'tests/fixtures/mock_common.py', work / 'mock_common.py', mode=0o600)
        atomic_copy(ROOT / 'scripts/release/files.py', work / 'mock_files.py', mode=0o600)
        self.identities = {}

    def install(self, label, *flags, setup=False):
        prefix = self.output / f'{label}-prefix'
        with raw_log(self.output / f'{label}-install.log') as log:
            log.write(f'\nInstall {label}: {flags!r}; setup={setup}\n'.encode()); log.flush()
            subprocess.run(['sh', str(self.bootstrap), '--from', str(self.assets),
                            '--prefix', str(prefix), *([] if setup else ['--no-setup']), *flags], check=True, env=self.env, stdout=log, stderr=subprocess.STDOUT, timeout=120)
        return prefix

    def verify(self, prefix, part):
        package, flags, names, variant = PARTS[part]
        component = 'hive' if part.startswith('hive') else 'comb'
        record = json.loads((prefix / 'share' / component / 'build.json').read_text())
        require(record['part'] == part and record['profile'] == 'release', record)
        require(record['command'] == ['cargo', 'build', '--release', '--locked', '-p', package, *flags, '--message-format=json-render-diagnostics'], record)
        for name in names:
            binary = prefix / 'bin' / name
            info = record['binaries'][name]
            require(not info['cargo_profile']['test'] and info['cargo_profile']['opt_level'] == '3', info)
            require(digest(binary) == info['sha256'], f'Installed {name} differs from plain release build')
            require(inspect_binary(binary, native_target()) == info['dependencies'], 'Installed dependency closure differs')
            version = subprocess.check_output([str(binary), '--version'], text=True, env=self.env).strip()
            require(version.endswith(f'({variant})'), version)
        self.identities[part] = {k: record[k] for k in ['source', 'target', 'binaries']}

    @contextmanager
    def daemon(self, prefix, label, additions=None):
        work = self.work / label; work.mkdir()
        instructions = work / 'role.md'; atomic_write_text(instructions, 'Deterministic installed release fixture.\n')
        config = {'auth_profiles': {'mock': {'provider': 'codex', 'method': 'subscription'}},
                  'roles': {'implementor': {'auth': 'mock', 'model': 'gpt-5.6-luna', 'effort': 'medium',
                            'instructions': str(instructions), 'lifetime': 'independent', 'sandbox': 'read-only'}}}
        config.update(additions or {})
        path = work / 'config.json'; write_json(path, config)
        command = [str(prefix / 'bin/hivectl'), '--config', str(path), '--state-dir', str(work / 'state'), '--codex-bin', str(self.provider)]
        mock_login(command, env=self.env)
        remote = [command[0], '--state-dir', str(work / 'state')]
        control = remote + ['--config', str(path)]
        call = Cli(remote, prefixes={'daemon': control, 'auth': control}, timeout=10)
        with running(command + ['daemon', 'run'], self.output / f'{label}-daemon.log', self.env) as process:
            wait_for(lambda: call('daemon', 'status'), f'{label} daemon startup', process)
            try:
                yield call, work, config, path
            finally:
                if process.poll() is None:
                    call('daemon', 'stop')
                    require(process.wait(timeout=20) == 0, f'{label} daemon shutdown failed')

    def turn(self, call, work, body):
        task = work / 'task.txt'; atomic_write_text(task, body)
        agent = call('spawn', 'implementor', '--cwd', str(work), '--body-file', str(task))['agent_id']
        def finished():
            events = event_pages(call, agent)
            outputs = [e['data']['text'] for e in events if isinstance(e['data'].get('text'), str)]
            if outputs and any(e['type'] == 'turn_finished' and e['data'].get('status') == 'completed' for e in events):
                return outputs[-1]
        result = wait_for(finished, 'installed agent turn')
        call('terminate', agent)
        return result

    def core(self):
        prefix = self.install('core', '--core'); self.verify(prefix, 'hive-core')
        with self.daemon(prefix, 'core') as (call, work, config, path):
            require(self.turn(call, work, 'reply:installed-core') == 'reply:installed-core', 'Core turn output missing')
        config['comb'] = {}
        write_json(path, config)
        rejected = subprocess.run(call.argv('auth', 'list'), text=True, capture_output=True, env=self.env, timeout=10, check=False)
        require(rejected.returncode != 0, 'Core accepted Comb configuration')
        error = json.loads(rejected.stdout)['error']
        require('comb' in error['message'].lower(), error)
        return {'mock_turn': 'completed', 'comb_config': error}

    def hosted(self):
        prefix = self.install('hosted', '--comb')
        self.verify(prefix, 'hive'); self.verify(prefix, 'comb')
        root = self.work / 'knowledge'; socket = self.work / 'comb.sock'
        local = [str(prefix / 'bin/comb'), '--root', str(root)]
        command = local + ['--socket', str(socket)]
        invoke = lambda *args: json_command(command + list(args), env=self.env)
        json_command(local + ['init'], env=self.env)
        with running(command + ['serve'], self.output / 'comb-server.log', self.env) as server:
            wait_for(lambda: socket.exists() and invoke('stats'), 'installed Comb server startup', server)
            credential = self.work / 'bridge.json'
            invoke('credential', 'create', 'installed-runtime', '--bridge', '--reviewer-role', 'implementor', '--out', str(credential))
            additions = {'comb': {'credential_file': str(credential)},
                         'comb_collectors': {'shared': {'credential_file': str(credential), 'sources': ['correction']}}}
            with self.daemon(prefix, 'hosted', additions) as (call, work, _, _):
                actions = [{'name': 'comb_put', 'arguments': {'slug': 'concepts/installed', 'markdown': '---\ntitle: Installed\n---\nRelease installation works.'}},
                           {'name': 'comb_search', 'arguments': {'query': 'installation', 'mode': 'text'}}]
                output = self.turn(call, work, 'comb:' + json.dumps(actions))
                require(output.startswith('comb-results:'), output)
                replies = json.loads(output.removeprefix('comb-results:'))
                require(len(replies) == 2 and all('error' not in r for r in replies), replies)
                require(replies[1]['mode'] == 'text' and replies[1]['results'][0]['slug'] == 'concepts/installed', replies[1])
                page = invoke('get', 'concepts/installed')
                require('Release installation works.' in json.dumps(page), page)
                admitted = call('evidence', 'correction', '--summary', 'Installed collector delivered public correction.', '--request-id', 'installed-correction')
                def delivered():
                    status = call('collectors', 'status')['collectors']
                    return status and status[0]['pending'] == 0 and invoke('stats')['inbox'] == 1
                wait_for(delivered, 'installed collector delivery', server)
                pages = json_command(command + ['list', '--prefix', 'inbox/'],
                                     env=self.env, allow_list=True)
                require(len(pages) == 1, pages)
                captured = invoke('get', pages[0]['slug'])
                require('Installed collector delivered public correction.' in json.dumps(captured), captured)
                require(admitted['destinations'] == {'shared': 'queued'}, admitted)
                return {'hosted_tool_replies': replies, 'collector_admission': admitted, 'inbox': pages}

    def wizard(self):
        """Exercise packaged setup and teardown in an isolated, manager-free home."""
        # Trusted launch images deliberately refuse HOME below /tmp.
        base = Path.home() / '.cache/lx-test'; base.mkdir(parents=True, exist_ok=True, mode=0o700)
        with tempfile.TemporaryDirectory(prefix='w-', dir=base) as directory, embedding_server() as url:
            home = Path(directory).resolve()
            previous = self.env
            self.env = dict(previous, HOME=str(home), XDG_CONFIG_HOME=str(home / 'cfg'),
                            XDG_STATE_HOME=str(home / 'st'), XDG_DATA_HOME=str(home / 'data'))
            try:
                return self._wizard(home, url)
            finally:
                self.env = previous

    def _wizard(self, home, url):
        workspace = home / 'repo'; workspace.mkdir()
        config, state = home / 'config.yaml', home / 'state'
        root, bridge, embedding = home / 'knowledge', home / 'bridge.json', home / 'embedding.json'
        write_json(embedding, {'backend': 'http', 'model': 'fixture', 'revision': 'pinned', 'url': url, 'dimensions': 2})
        global_args = ['--config', str(config), '--state-dir', str(state), '--codex-bin', str(self.provider)]
        answers = ['--yes', '--provider', 'codex', '--codex-profile', 'mock', '--login', 'skip',
                   '--codex-model', 'gpt-5.6-luna', '--codex-effort', 'medium', '--no-services',
                   '--comb-root', str(root), '--bridge-file', str(bridge), '--embedding-config', str(embedding)]
        label, variant = 'wizard', '--comb'
        prefix = self.install(label, variant, '--yes', '--', *global_args, *answers, setup=True)
        self.verify(prefix, 'hive'); self.verify(prefix, 'comb')
        command = [str(prefix / 'bin/hivectl'), *global_args]
        remote = [command[0], '--state-dir', str(state)]
        control = remote + ['--config', str(config)]
        routing = Cli(remote, prefixes={'daemon': control, 'setup': command, 'doctor': command})
        call = lambda *args: json_command(routing.argv(*args), env=self.env)
        mock_login(command, env=self.env)
        pinned = [config, bridge, root / '.comb/serve.json']
        before = [(p.read_bytes(), p.stat().st_mtime_ns) for p in pinned]
        again = call('setup', *answers)
        require(again['configured'] and not again['changed'], again)
        require(before == [(p.read_bytes(), p.stat().st_mtime_ns) for p in pinned], 'Identical setup rewrote owned settings')
        conflicting = answers.copy(); conflicting[conflicting.index('--codex-model') + 1] = 'conflicting-model'
        conflict = subprocess.run(command + ['setup', *conflicting], env=self.env,
                                  capture_output=True, text=True, timeout=30, check=False)
        require(conflict.returncode != 0 and config.read_bytes() == before[0][0], 'Setup conflict changed configuration')
        require(not list(home.glob('config.yaml.backup*')), 'Conflict created a backup')
        comb = [str(prefix / 'bin/comb'), '--root', str(root)]
        with running(comb + ['serve'], self.output / f'{label}-comb.log', self.env) as server:
            wait_for(lambda: root.joinpath('.comb/service.sock').exists() and json_command(comb + ['stats'], env=self.env), 'wizard Comb startup', server)
            with running(command + ['daemon', 'run'], self.output / f'{label}-daemon.log', self.env) as daemon:
                wait_for(lambda: call('daemon', 'status'), 'wizard daemon startup', daemon)
                doctor = call('doctor')
                require(all(c['status'] != 'error' for c in doctor['checks']), doctor)
                actions = [{'name': 'comb_put', 'arguments': {'slug': 'concepts/wizard', 'markdown': '---\ntitle: Wizard\n---\nPackaged setup works.'}},
                           {'name': 'comb_search', 'arguments': {'query': 'Packaged', 'mode': 'text'}}]
                agent = call('spawn', '--cwd', str(workspace), '--body', 'comb:' + json.dumps(actions))['agent_id']
                def finished():
                    events = event_pages(call, agent)
                    outputs = [e['data']['text'] for e in events if isinstance(e['data'].get('text'), str)]
                    return outputs[-1] if outputs and any(e['type'] == 'turn_finished' and e['data'].get('status') == 'completed' for e in events) else None
                output = wait_for(finished, 'wizard default agent Comb turn', daemon)
                require(output.startswith('comb-results:'), output)
                replies = json.loads(output.removeprefix('comb-results:'))
                require(len(replies) == 2 and all('error' not in r for r in replies), replies)
                require(replies[1]['results'][0]['slug'] == 'concepts/wizard', replies)
                call('terminate', agent)
                def indexed():
                    found = json_command(comb + ['search', 'Packaged', '--mode', 'vector'], env=self.env)
                    require(found['mode'] == 'vector', found)
                    return found if found['results'] and found['results'][0]['slug'] == 'concepts/wizard' else None
                wait_for(indexed, 'packaged HTTP embedding background indexing', server)
                call('daemon', 'stop'); require(daemon.wait(timeout=20) == 0, 'Wizard daemon stop failed')
        # Upgrade without setup preserves selected data, then plain removal preserves it too.
        retained = {p: p.read_bytes() for p in [*pinned, root / 'concepts/wizard.md', *state.joinpath('profiles').rglob('*')] if p.is_file()}
        require(any(p.is_relative_to(state / 'profiles') for p in retained), 'No profile evidence captured')
        self.install(label, variant, '--yes')
        require(all(p.read_bytes() == data for p, data in retained.items()), 'Upgrade changed retained data')
        remove = ['sh', str(self.bootstrap), '--from', str(self.assets), '--prefix', str(prefix),
                  '--uninstall', '--comb', '--yes']
        purge = remove + ['--purge', '--config', str(config), '--state-dir', str(state),
                          '--comb-root', str(root), '--bridge-file', str(bridge)]
        log = self.output / f'{label}-removal.log'
        with raw_log(log) as stream:
            subprocess.run(remove, env=self.env, check=True, stdout=stream, stderr=subprocess.STDOUT, timeout=120)
            require(all(p.read_bytes() == data for p, data in retained.items()), 'Plain removal changed retained data')
            subprocess.run(purge, env=self.env, check=True, stdout=stream, stderr=subprocess.STDOUT, timeout=120)
            subprocess.run(purge, env=self.env, check=True, stdout=stream, stderr=subprocess.STDOUT, timeout=120)
        require(not config.exists() and not bridge.exists() and not prefix.joinpath('lib/hive').exists(), 'Full purge left selected payload')
        retained_after = {p: data for p, data in retained.items() if p == root / 'concepts/wizard.md' or p.is_relative_to(state / 'profiles')}
        require(all(p.read_bytes() == data for p, data in retained_after.items()), 'Purge changed retained knowledge/auth')
        # A new standalone install can explicitly remove the retained root.
        self.install(label, '--standalone-comb', '--yes')
        explicit = ['sh', str(self.bootstrap), '--from', str(self.assets), '--prefix', str(prefix),
                    '--uninstall', '--standalone-comb', '--purge', '--yes', '--comb-root', str(root),
                    '--purge-comb-root', str(root)]
        with raw_log(log) as stream:
            subprocess.run(explicit, env=self.env, check=True, stdout=stream, stderr=subprocess.STDOUT, timeout=120)
        require(not root.exists(), 'Explicit knowledge-root purge did not remove it')
        return {'setup': 'configured', 'identical': 'unchanged', 'conflict': 'preserved', 'doctor': doctor,
                'default_agent_comb_tools': replies, 'http_vector_search': 'indexed', 'upgrade': 'preserved', 'plain_uninstall': 'retained',
                'purge': 'retained knowledge/auth; repeat no-op', 'explicit_root_purge': 'removed'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve(); create_directory(output)
    report = {'target': native_target(), 'profile': 'release', 'checks': {}}
    # Keep sockets well within the Unix path limit, independently of checkout path.
    with tempfile.TemporaryDirectory(prefix='hive-i-', dir='/tmp') as scratch:
        checks = InstalledChecks(args.assets.resolve(), output, Path(scratch).resolve())
        for name in ['core', 'hosted', 'wizard']:
            began = time.monotonic()
            with phase('installed_' + name):
                report['checks'][name] = getattr(checks, name)()
            report['checks'][name]['seconds'] = time.monotonic() - began
            print(f'{name}: passed', flush=True)
            report['artifacts'] = checks.identities
            write_json(output / 'results.json', report)
    print(output / 'results.json')


if __name__ == '__main__':
    raise SystemExit(entry(main, name='install'))
