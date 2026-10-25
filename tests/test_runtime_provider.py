"""Deterministic multiplexing checks for the runtime integration fixture."""

import json
import os
import queue
import subprocess
import sys
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


class RuntimeProviderTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="hl-fixture-")
        self.addCleanup(self.directory.cleanup)
        self.process = subprocess.Popen(
            [sys.executable, str(Path(__file__).parent / "fixtures/runtime_provider.py")],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=dict(os.environ, CODEX_HOME=self.directory.name),
        )
        self.frames = queue.Queue()
        self.reader = threading.Thread(target=self.read_frames, daemon=True)
        self.reader.start()
        self.addCleanup(self.close)
        self.sequence = 0

    def read_frames(self):
        for line in self.process.stdout:
            self.frames.put(json.loads(line))

    def close(self):
        self.process.stdin.close()
        try:
            code = self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise
        finally:
            self.reader.join(timeout=5)
            self.process.stdout.close()
        error = self.process.stderr.read()
        self.process.stderr.close()
        self.assertEqual(code, 0, error)

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()

    def receive(self):
        return self.frames.get(timeout=5)

    def call(self, method, params):
        self.sequence += 1
        self.send(dict(id=self.sequence, method=method, params=params))
        response = self.receive()
        self.assertEqual(response["id"], self.sequence)
        return response

    def start(self, name):
        params = dict(
            developerInstructions=f"You are Hive agent {name}, parent nobody.",
            model="gpt-5.6-terra",
            dynamicTools=[{"name": "fixture"}],
            config=dict(model_reasoning_effort="high", mcp_servers={}),
        )
        return self.call("thread/start", params)["result"]["thread"]["id"]

    def turn(self, thread, body):
        response = self.call(
            "turn/start",
            dict(
                threadId=thread,
                model="gpt-5.6-terra",
                effort="high",
                input=[dict(text=f"Hive message seed from human:\n{body}")],
            ),
        )
        turn = response["result"]["turn"]["id"]
        self.notification("turn/started", thread, turn)
        return turn

    def notification(self, method, thread, turn):
        frame = self.receive()
        self.assertEqual(frame["method"], method)
        self.assertEqual(frame["params"]["threadId"], thread)
        self.assertEqual(frame["params"]["turn"]["id"], turn)

    def test_interleaved_start_turn_cleanup_and_resume_keep_conversation_identity(self):
        first = self.start("first")
        second = self.start("second")
        first_turn = self.turn(first, "idle")
        self.notification("turn/completed", first, first_turn)
        second_turn = self.turn(second, "hold")
        # A late interrupt on the completed first turn must not end the second.
        self.assertIn("error", self.call("turn/interrupt", dict(threadId=first, turnId=first_turn)))
        self.assertIn("error", self.call("turn/interrupt", dict(threadId=second, turnId="stale")))
        for method in (
            "thread/backgroundTerminals/clean",
            "thread/backgroundTerminals/list",
            "thread/unsubscribe",
        ):
            self.assertIn("result", self.call(method, dict(threadId=first)))
        resumed = self.call(
            "thread/resume",
            dict(threadId=first, model="gpt-5.6-terra", config=dict(model_reasoning_effort="high", mcp_servers={})),
        )
        self.assertEqual(resumed["result"]["thread"]["id"], first)
        self.assertIn(
            "result", self.call("turn/interrupt", dict(threadId=second, turnId=second_turn))
        )
        self.notification("turn/completed", second, second_turn)
        resumed_turn = self.turn(first, "idle")
        self.notification("turn/completed", first, resumed_turn)

    def test_reverse_tool_replies_finish_their_own_turns(self):
        first = self.start("first")
        second = self.start("second")
        pending = []
        for thread in (first, second):
            turn = self.turn(thread, "complete")
            tool = self.receive()
            self.assertEqual(tool["params"]["threadId"], thread)
            pending.append((thread, turn, tool["id"]))
        for thread, turn, callback in reversed(pending):
            self.send(dict(id=callback, result=dict(success=True, contentItems=[])))
            self.notification("turn/completed", thread, turn)
            # A new turn can finish while another conversation still awaits its tool reply.
            next_turn = self.turn(thread, "reply:after-tool")
            self.assertEqual(
                self.receive(),
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": thread,
                        "turnId": next_turn,
                        "item": {
                            "type": "agentMessage",
                            "text": "reply:after-tool",
                            "phase": "final_answer",
                        },
                    },
                },
            )
            self.notification("turn/completed", thread, next_turn)
            saved = json.loads((Path(self.directory.name) / f"{thread}.json").read_text())
            self.assertEqual(saved["last_turn"], next_turn)
            self.assertEqual(saved["history"], ["complete", "reply:after-tool"])


class PublicationTests(unittest.TestCase):
    def test_concurrent_native_fork_requests_claim_each_rejection_once(self):
        provider = Path(__file__).parent / 'fixtures/runtime_provider.py'
        for round_number in range(4):
            with self.subTest(round=round_number), tempfile.TemporaryDirectory() as temp:
                home = Path(temp)
                count = 128
                (home / 'source.json').write_text(json.dumps({'last_turn': 'source-turn'}))
                (home / 'runtime-scenario.json').write_text(json.dumps(
                    {'fork_rejections': [f'rejection-{i}' for i in range(count)]}))
                request = dict(method='thread/fork', params=dict(
                    threadId='source', lastTurnId='source-turn', excludeTurns=True, ephemeral=False))
                data = ''.join(json.dumps(dict(id=i, **request)) + '\n' for i in range(16))
                def run(_):
                    result = subprocess.run([sys.executable, str(provider)], input=data, text=True,
                        capture_output=True, timeout=15, env=dict(os.environ, CODEX_HOME=temp), check=False)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    return [json.loads(line)['error']['message'] for line in result.stdout.splitlines()]
                with ThreadPoolExecutor(max_workers=8) as pool:
                    messages = [message for rows in pool.map(run, range(8)) for message in rows]
                self.assertEqual(len(messages), count)
                self.assertEqual(set(messages), {f'rejection-{i}' for i in range(count)})
                self.assertEqual(int((home / 'fork-rejected').read_text()), count)

    def test_copied_auth_peer_publishes_complete_pid_before_observation(self):
        root = Path(__file__).resolve().parents[1]
        fixtures = root / 'tests/fixtures'
        with tempfile.TemporaryDirectory() as temp:
            work = Path(temp)
            (work / 'codex-schema.json').write_bytes((fixtures / 'codex-schema.json').read_bytes())
            (work / 'mock_common.py').write_bytes((fixtures / 'mock_common.py').read_bytes())
            (work / 'mock_files.py').write_bytes((root / 'scripts/release/files.py').read_bytes())
            provider = work / 'provider.py'
            scenario = work / 'scenario.json'
            provider.write_text(f'SCENARIO = {str(scenario)!r}\nPROVIDER = "codex"\n'
                                + (fixtures / 'provider.py').read_text())
            env = {k: v for k, v in os.environ.items() if k not in ('PYTHONPATH', 'HIVE_ROOT')}
            env['CODEX_HOME'] = temp
            for iteration in range(12):
                ready, release = work / f'ready-{iteration}', work / f'release-{iteration}'
                scenario.write_text(json.dumps(dict(ready=str(ready), hang=True, stop_file=str(release))))
                child = subprocess.Popen([sys.executable, str(provider)], cwd=work, env=env,
                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    import time
                    deadline = time.monotonic() + 10
                    while not ready.exists() and time.monotonic() < deadline:
                        self.assertIsNone(child.poll(), 'peer exited before publication')
                        time.sleep(.001)
                    self.assertEqual(int(ready.read_text()), child.pid)
                    release.touch()
                    _, stderr = child.communicate(timeout=10)
                    self.assertEqual(child.returncode, 0, stderr)
                finally:
                    if child.poll() is None:
                        child.kill()
                    child.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
