"""Generate upgrade data through producer binaries and inspect it with readers."""
import json

from .acceptance import event_pages, require
from .files import atomic_write_text


def completed(call, agent):
    value = call('status', agent)['agent']
    turn = value.get('last_finished_turn')
    if value['state'] == 'idle' and turn:
        require(turn['outcome'] == 'completed', 'Corpus turn failed')
        return value
    return None


class StateCorpus:
    def __init__(self, work, inputs):
        self.work, self.inputs = work, inputs

    def seed(self, call, wait):
        large = self.inputs['output']['text'] * self.inputs['output']['repetitions']
        self.source = call('spawn', 'implementor', '--cwd', str(self.work), '--body', 'reply:' + large)['agent_id']
        self.source_before = wait(lambda: completed(call, self.source), 'artifact source turn')
        artifact = next(e['data']['artifact']['id'] for e in event_pages(call, self.source) if 'artifact' in e['data'])
        self.output_args = ('output', artifact, '--offset', '0', '--limit', '65536')
        self.output = call(*self.output_args)
        require(large in json.dumps(self.output), 'Corpus output was not retained')
        self.child = call('spawn', '--from', self.source, '--body', self.inputs['branch_body'])['agent_id']
        self.before = wait(lambda: completed(call, self.child), 'branch turn')
        require(self.before['branch_phase'] == 'bound' and self.before['thread_id'] != self.source_before['thread_id'], 'Corpus branch was not bound')
        call('stop', self.child)
        wait(lambda: call('status', self.child)['agent']['state'] == 'stopped', 'corpus pause')
        attachment = self.work / 'attachment.txt'
        atomic_write_text(attachment, self.inputs['attachment'], mode=0o600)
        self.queued = []
        for index, body in enumerate(self.inputs['queued']):
            arguments = ('send', self.child, '--body', body, '--attach', str(attachment), '--request-id', 'upgrade-input-' + str(index))
            self.queued.append((arguments, call(*arguments)))
        attachment.unlink()

    def verify(self, call, wait, state):
        restored = call('status', self.child)['agent']
        for field in ('thread_id', 'auth_generation', 'branched_from_agent_id', 'branch_phase'):
            require(self.before[field] is not None and restored[field] == self.before[field], 'Upgrade lost branch/native binding: ' + field)
        require(call('status', self.source)['agent']['thread_id'] == self.source_before['thread_id'], 'Upgrade lost source conversation')
        require(call(*self.output_args) == self.output, 'Upgrade lost retained output')
        for arguments, response in self.queued:
            require(call(*arguments) == response, 'Upgrade lost queued request deduplication')
        call('resume', self.child)
        last = self.queued[-1][1]['message_id']
        wait(lambda: (value := completed(call, self.child)) and value['last_finished_turn']['message'] == last, 'upgraded attachment turns')
        history = event_pages(call, self.child)
        messages = call('messages', self.child)['messages']
        inputs = [json.loads(line) for path in (state / 'profiles').rglob('inputs.jsonl') for line in path.read_text().splitlines()]
        for arguments, response in self.queued:
            matching = [e for e in history if e['type'] == 'turn_finished' and e['data'].get('message_id') == response['message_id']]
            require(len(matching) == 1 and matching[0]['data']['status'] == 'completed', 'Queued turn was not completed exactly once')
            delivery = [m for m in messages if m['id'] == response['message_id']]
            require(len(delivery) == 1 and len(delivery[0]['attachments']) == 1, 'Upgrade lost attachment metadata')
            frames = [f for f in inputs if arguments[3] in json.dumps(f)]
            require(len(frames) == 1 and frames[0]['thread'] == self.before['thread_id'], 'Native input replayed or changed thread')
            snapshot = delivery[0]['attachments'][0]['source']['path']
            from pathlib import Path
            require(Path(snapshot).read_text() == self.inputs['attachment'], 'Retained attachment contents changed')


class KnowledgeCorpus:
    def __init__(self, work, inputs):
        self.work, self.inputs, self.replays = work, inputs, []

    def seed(self, invoke, untrusted):
        self.slugs = []
        for key, page in self.inputs['pages'].items():
            self.slugs.append(page['slug'])
            if key == 'untrusted':
                untrusted(page)
            else:
                source = self.work / (key + '.md')
                atomic_write_text(source, page['markdown'])
                invoke('put', page['slug'], '--file', str(source), '--request-id', 'seed-' + key)
        note, target, proposal = (self.inputs['pages'][key]['slug'] for key in ('note', 'trusted', 'proposal'))
        summary = self.work / 'summary.md'
        atomic_write_text(summary, self.inputs['summary'])
        fold = ('--run', 'synthetic-release', 'consolidate', note, target,
                '--note-hash', invoke('get', note)['hash'], '--target-hash', invoke('get', target)['hash'],
                '--summary-file', str(summary), '--request-id', 'fold')
        decide = ('decide', proposal, 'reject', '--reason', self.inputs['reason'],
                  '--expected-hash', invoke('get', proposal)['hash'], '--request-id', 'decide')
        capture = ('capture', self.inputs['capture'], '--request-id', 'capture')
        for args in (fold, decide, capture):
            self.replays.append((args, invoke(*args)))
        self.documents = {slug: invoke('get', slug) for slug in self.slugs}
        for page in self.inputs['pages'].values():
            require(self.documents[page['slug']]['unverified'] == page['unverified'], 'Corpus trust expectation not exercised')

    def searches(self, invoke):
        result = []
        for query in self.inputs['queries']:
            for mode in ('text', 'vector', 'hybrid'):
                found = invoke('search', query, '--mode', mode, '--before', '2100-01-01T00:00:00Z')
                require(found['mode'] == mode and bool(found['results']), 'Corpus search missing requested mode or results')
                if mode != 'text':
                    coverage = found['coverage']
                    require(coverage['pending'] == 0 and coverage['rejected'] == 0 and coverage['embedded'] == coverage['total'],
                            'Corpus search requires complete deterministic embedding coverage')
                result.append(found)
        return result

    def verify(self, invoke, before_search):
        require({slug: invoke('get', slug) for slug in self.slugs} == self.documents, 'Upgrade lost page content, trust or decisions')
        require(self.searches(invoke) == before_search, 'Upgrade changed text/vector/hybrid results')
        for args, original in self.replays:
            replay = invoke(*args)
            if args[2] == 'consolidate':
                for key in ('note', 'target'):
                    require(replay[key]['replayed'], 'Consolidation receipt lost')
                    replay[key]['replayed'] = original[key]['replayed']
            else:
                require(replay['replayed'], 'Mutation receipt lost')
                replay['replayed'] = original['replayed']
            require(replay == original, 'Receipt replay changed original result')
