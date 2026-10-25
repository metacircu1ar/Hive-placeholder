import json
import os
import pathlib
import signal
import sys
import time

from mock_common import load_scenario, wait_for_gate, publish_text, append_line, claim_counter

# The Rust fixture prepends these bindings to each provider executable.
SCENARIO: str
PROVIDER: str

scenario = load_scenario(SCENARIO, required=True)
from mock_common import schema_probe
if schema_probe(sys.argv):
    raise SystemExit()

if "--version" in sys.argv:
    if scenario.get("version_stray"):
        publish_text(pathlib.Path(os.environ["CLAUDE_CONFIG_DIR"]).parent.joinpath("stray"), "retained")
    if scenario.get("version_corrupt_index"):
        publish_text(pathlib.Path(scenario["version_corrupt_index"]), '{"broken":true}')
    if scenario.get("version_fail"):
        sys.exit(7)
    print(
        scenario.get(
            "version", "codex-cli 0.154.0" if PROVIDER == "codex" else "2.1.285 (Claude Code)"
        )
    )
    sys.exit(0)
home = pathlib.Path(os.environ["CODEX_HOME" if PROVIDER == "codex" else "CLAUDE_CONFIG_DIR"])
marker = home / "fake-login-complete"
if "login" in sys.argv:
    assert sys.stdin.isatty() and sys.stdout.isatty()
    assert os.tcgetpgrp(0) == os.getpgrp(), "login not in foreground process group"
    if scenario.get("login_ignore_signals"):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    elif scenario.get("login_ignore_sigint"):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
    elif scenario.get("login_slow_sigint"):
        cancelled_at = None

        def slow_cancel(signum, frame):
            global cancelled_at
            if cancelled_at is None:
                cancelled_at = time.monotonic()

        signal.signal(signal.SIGINT, slow_cancel)
    print("fake-login: enter confirmation", flush=True)
    if scenario.get("login_ignore_signals") or scenario.get("login_ignore_sigint"):
        time.sleep(20)
    if scenario.get("login_slow_sigint"):
        while cancelled_at is None or time.monotonic() - cancelled_at < 0.3:
            time.sleep(0.01)
        sys.exit(130)
    if scenario.get("login_raw_enter"):
        import tty

        tty.setraw(0)
        sys.stdout.write("raw-ready\n")
        sys.stdout.flush()
        data = bytearray()
        while not data.endswith(b"\r"):
            data.extend(os.read(0, 1))
        assert data == b"confirm\r", repr(data)
    else:
        assert input() == "confirm"
    if scenario.get("login_failure"):
        sys.exit(1)
    if not scenario.get("login_no_credentials"):
        publish_text(marker, "done")
    if scenario.get("login_detached_stop"):
        import subprocess

        subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import pathlib,time,sys; stop=pathlib.Path(sys.argv[1]); deadline=time.monotonic()+15\nwhile not stop.exists() and time.monotonic()<deadline: time.sleep(.01)",
                scenario["login_detached_stop"],
            ],
            close_fds=False,
            start_new_session=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    if scenario.get("login_notify_saved"):
        print("fake-login: saved", flush=True)
        # Keep the login alive until the harness hangs up its terminal and Hive
        # forwards cancellation. A short sleep races the harness on a busy host.
        wait_for_gate(None, timeout=20)
        sys.exit("login was never cancelled after saving credentials")
    sys.exit(0)
if scenario.get("close_fds"):
    os.closerange(3, 1024)
if scenario.get("ready"):
    publish_text(pathlib.Path(scenario["ready"]), str(os.getpid()))
if scenario.get("evidence"):
    publish_text(pathlib.Path(scenario["evidence"]), json.dumps(
            {
                "environment": dict(os.environ),
                "cwd": os.getcwd(),
                "config_sources": "--setting-sources" in sys.argv
                and sys.argv[sys.argv.index("--setting-sources") + 1] == "",
            }
        ))
if scenario.get("hang"):
    stop = pathlib.Path(scenario["stop_file"]) if scenario.get("stop_file") else None
    wait_for_gate(stop, timeout=20)
if scenario.get("flood"):
    sys.stdout.write("x" * 5000000)
    sys.stdout.flush()
state = scenario.get("state", "authenticated" if marker.exists() else "missing")
if marker.exists():
    state = scenario.get("post_login_state", state)
    if scenario.get("post_login_corrupt_index"):
        publish_text(pathlib.Path(scenario["post_login_corrupt_index"]), '{"broken":true}')
if state == "malformed":
    print("secret-canary malformed output", flush=True)
    sys.exit(0)
if PROVIDER == "claude":
    print(
        json.dumps(
            {
                "loggedIn": state not in ("missing", "reauthentication_required"),
                "authMethod": {
                    "missing": "none",
                    "authenticated": "claude.ai",
                    "reauthentication_required": "claude.ai",
                    "api": "api_key",
                }.get(state, "unknown"),
                "apiProvider": "firstParty",
                "email": scenario.get("email", "fixture@example.test"),
            }
        ),
        flush=True,
    )
    sys.exit(1 if state in ("missing", "reauthentication_required") else 0)
for line in sys.stdin:
    request = json.loads(line)
    if request["method"] == "initialize":
        response = {}
    elif request["method"] == "model/list":
        response = {"data": scenario.get("models", [
            {"id": "fixture-first", "model": "fixture-first", "supportedReasoningEfforts": [{"reasoningEffort": "low"}]},
            {"id": "fixture-default", "model": "fixture-default", "isDefault": True, "defaultReasoningEffort": "high", "supportedReasoningEfforts": [{"reasoningEffort": "low"}, {"reasoningEffort": "high"}]}
        ]), "nextCursor": None}
    elif request["method"] == "account/read":
        response = {
            "account": (
                None
                if state == "missing"
                else {
                    "type": "apiKey" if state == "api" else "chatgpt",
                    "email": scenario.get("email", "fixture@example.test"),
                }
            ),
            "requiresOpenaiAuth": True,
        }
    else:
        continue
    print(json.dumps({"id": request["id"], "result": response}), flush=True)
if scenario.get("shutdown_marker"):
    publish_text(pathlib.Path(scenario["shutdown_marker"]), "graceful EOF")
if scenario.get("shutdown_hang"):
    time.sleep(20)
