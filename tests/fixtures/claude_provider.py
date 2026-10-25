"""Deterministic native Claude stream protocol; never calls a model service."""

import json
import os
import shlex
import signal
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path

from mock_common import run_stdio_mcp, load_scenario, wait_for_gate, publish_text, append_line, claim_counter
from mock_common import INPUT_RECORD_LIMIT

home = Path(os.environ["CLAUDE_CONFIG_DIR"])


def scenario():
    # Test commands can change settings while this native process remains resident.
    return load_scenario(home / "runtime-scenario.json")


args = sys.argv[1:]


def option(name):
    return args[args.index(name) + 1]


def emit(frame):
    print(json.dumps(frame), flush=True)


def control(request, response):
    emit(
        {
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request, "response": response},
        }
    )


if "--version" in args:
    print(scenario().get("provider_version", "2.1.287") + " (Claude Code)")
    raise SystemExit()
if "auth" in args:
    print(json.dumps({"loggedIn": True, "authMethod": "claude.ai", "apiProvider": "firstParty"}))
    raise SystemExit()

assert "app-server" not in args
assert option("--setting-sources") == ""
if scenario().get("api_key"):
    assert os.environ.get("ANTHROPIC_API_KEY", "").startswith("private-api-")
    assert "ANTHROPIC_AUTH_TOKEN" not in os.environ
    assert "OPENAI_API_KEY" not in os.environ and "CODEX_API_KEY" not in os.environ
    assert os.environ["ANTHROPIC_API_KEY"] not in args
native_model = (
    "claude-haiku-4-5"
    if "--model" in args and option("--model") in ("haiku", "claude-haiku-4-5")
    else "claude-sonnet-5"
)
fork = "--fork-session" in args
session = (
    option("--session-id")
    if fork
    else (
        option("--resume")
        if "--resume" in args
        else option("--session-id") if "--session-id" in args else None
    )
)
saved = home / ("mock-session-" + str(session))
if ("--resume" in args and not fork and not saved.exists()) or (
    "--session-id" in args and saved.exists()
):
    emit(
        {
            "type": "result",
            "session_id": "native-startup-id",
            "subtype": "error_during_execution",
            "is_error": True,
            "num_turns": 0,
        }
    )
    raise SystemExit(1)
if fork:
    assert (home / ("mock-session-" + option("--resume"))).exists()
history_file = home / ("mock-history-" + str(session))
history = json.loads(history_file.read_text()) if history_file.exists() else []
if fork:
    history = json.loads((home / ("mock-history-" + option("--resume"))).read_text())
if session:
    assert os.environ["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"] == "1"
    settings = json.loads(option("--settings"))
    assert settings["sandbox"]["enabled"] and settings["sandbox"]["failIfUnavailable"]
    assert not settings["sandbox"]["allowUnsandboxedCommands"]
    assert "--permission-prompt-tool" in args and "--replay-user-messages" in args
    append_line(home / "mock-launches.jsonl", json.dumps(
                {
                    "session": session,
                    "resume": "--resume" in args,
                    "pid": os.getpid(),
                    "model": native_model,
                    "effort": option("--effort") if "--effort" in args else "none",
                    "settings": settings,
                    "environment": {
                        key: os.environ.get(key)
                        for key in (
                            "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
                            "MAX_THINKING_TOKENS",
                            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                            "MAX_MCP_OUTPUT_TOKENS",
                        )
                    },
                    "instructions": Path(option("--append-system-prompt-file")).read_text(),
                }
            )
            + "\n")

native_history_file = home / "projects" / "mock-native" / (str(session) + ".jsonl")
native_history = []
(home / "projects").mkdir(mode=0o700, exist_ok=True)
native_history_file.parent.mkdir(mode=0o700, exist_ok=True)
native_source = native_history_file
if fork:
    native_source = native_source.with_name(option("--resume") + ".jsonl")
if native_source.exists():
    native_history = [json.loads(line) for line in native_source.read_text().splitlines()]
if fork and scenario().get("branch_bad_history"):
    native_history = []

turn = None
mode = None
counter = 0
pending = {}
comb_actions = []
comb_results = []
queued_lines = []
shell_process = None
agent = (
    Path(option("--append-system-prompt-file")).read_text().split("agent ", 1)[1].split(",", 1)[0]
    if session
    else None
)


def finish(interrupted=False):
    global turn
    emit(
        {
            "type": "result",
            "session_id": session,
            "subtype": "success",
            "is_error": False,
            "terminal_reason": "aborted_tools" if interrupted else "completed",
            "user_message_uuid": turn,
        }
    )
    turn = None


def tool(name, arguments, continuation, native_id=None):
    global counter
    counter += 1
    outer = "control-" + str(counter) if native_id is None else "comb-" + str(native_id)
    pending[outer] = continuation
    emit(
        {
            "type": "control_request",
            "request_id": outer,
            "request": {
                "subtype": "mcp_message",
                "server_name": "hive",
                "message": {
                    "jsonrpc": "2.0",
                    "id": "mcp-" + str(counter) if native_id is None else "comb-mcp-" + str(native_id),
                    "method": "tools/call",
                    "params": {"name": name, "arguments": arguments},
                },
            },
        }
    )


def comb_next():
    if comb_actions:
        action = comb_actions.pop(0)
        tool(action["name"], action.get("arguments", {}), "comb-result", action.get("native_id"))
    else:
        complete("comb-results:" + json.dumps(comb_results))


def complete(text):
    message = {"model": native_model, "content": [{"type": "text", "text": text}]}
    if scenario().get("live_deltas"):
        # Partial stream events precede the aggregated assistant message.
        message["id"] = "msg_" + str(turn)
        def partial(event):
            emit({"type": "stream_event", "session_id": session, "parent_tool_use_id": None,
                  "uuid": turn, "event": event})
        partial({"type": "message_start", "message": {"model": native_model, "id": message["id"]}})
        chunk = scenario().get("delta_chars", 3)
        partial({"type": "content_block_delta", "index": 0,
                 "delta": {"type": "thinking_delta", "thinking": "Planning."}})
        for start in range(0, len(text), chunk):
            partial({"type": "content_block_delta", "index": 1,
                     "delta": {"type": "text_delta", "text": text[start:start + chunk]}})
        partial({"type": "content_block_delta", "index": 1,
                 "delta": {"type": "input_json_delta", "partial_json": "{}"}})
    emit(
        {
            "type": "assistant",
            "session_id": session,
            "parent_tool_use_id": None,
            "message": message,
        }
    )
    finish()


while True:
    line = queued_lines.pop(0) if queued_lines else sys.stdin.readline()
    if not line:
        break
    frame = json.loads(line)
    kind = frame["type"]
    if kind == "control_request":
        request = frame["request"]
        if request["subtype"] == "initialize":
            if not session and scenario().get("catalog_gate"):
                release = Path(scenario()["catalog_gate"])
                publish_text(home / "catalog-pid", str(os.getpid()))
                wait_for_gate(
                    release, timeout=30, timeout_error=RuntimeError("catalog gate timed out")
                )
            control(
                frame["request_id"],
                {
                    "models": [
                        {
                            "value": "sonnet",
                            "resolvedModel": "claude-sonnet-5",
                            "displayName": "Sonnet",
                            "description": "Deterministic mock",
                            "supportedEffortLevels": ["low", "medium", "high"],
                        },
                        {
                            "value": "haiku",
                            "resolvedModel": "claude-haiku-4-5",
                            "displayName": "Haiku",
                            "description": "Deterministic mock",
                            "supportedEffortLevels": ["low", "medium", "high"],
                        },
                    ],
                    "account": scenario().get("api_account", (
                        {"apiProvider": "firstParty", "apiKeySource": "ANTHROPIC_API_KEY"}
                        if "ANTHROPIC_API_KEY" in os.environ else
                        {"subscriptionType": "Claude Team", "apiProvider": "firstParty"})),
                },
            )
        elif request["subtype"] == "get_settings":
            if scenario().get("settings_gate"):
                release = Path(scenario()["settings_gate"])
                publish_text(home / "settings-waiting", str(os.getpid()))
                wait_for_gate(
                    release, timeout=10, timeout_error=RuntimeError("settings gate timed out")
                )
            control(
                frame["request_id"],
                {
                    "applied": {
                        "model": native_model,
                        "effort": (
                            "low"
                            if scenario().get("mismatch_effort")
                            else option("--effort") if "--effort" in args else None
                        ),
                    }
                },
            )
            if scenario().get("backpressure"):
                prefix = os.read(sys.stdin.fileno(), 1)
                assert prefix == b"{"
                pending["backpressure"] = "backpressure"
                emit(
                    {
                        "type": "control_request",
                        "request_id": "backpressure",
                        "request": {
                            "subtype": "mcp_message",
                            "server_name": "hive",
                            "message": {"jsonrpc": "2.0", "id": 42, "method": "ping"},
                        },
                    }
                )
                time.sleep(0.2)
                queued_lines.append(prefix.decode() + sys.stdin.readline())
        elif request["subtype"] == "interrupt":
            append_line(home / "interrupts.jsonl", json.dumps({"turn": turn}) + "\n")
            if turn and scenario().get("complete_before_interrupt"):
                finish()
            control(frame["request_id"], {"still_queued": [], "cancelled": []})
            if turn:
                if shell_process is not None:
                    emit(
                        {
                            "type": "user",
                            "session_id": session,
                            "message": {
                                "content": [
                                    {
                                        "type": "tool_result",
                                        "tool_use_id": "shell-tool",
                                        "content": "interrupted",
                                        "is_error": True,
                                    }
                                ]
                            },
                        }
                    )
                if mode in ("error-interrupt", "error-interrupt-exit"):
                    emit(
                        {
                            "type": "result",
                            "session_id": session,
                            "subtype": "error_during_execution",
                            "is_error": True,
                            "num_turns": 1,
                        }
                    )
                    turn = None
                    if mode == "error-interrupt-exit":
                        raise SystemExit(1)
                    mode = None
                else:
                    finish(interrupted=True)
            if shell_process is not None:
                time.sleep(0.15)  # native result may precede actual shell cleanup
                os.kill(int(Path("helper.pid").read_text()), signal.SIGTERM)
                shell_process.communicate(timeout=5)
                shell_process = None
        else:
            raise AssertionError(request)
    elif kind == "user":
        assert turn is None
        turn = frame["uuid"]
        assert frame["session_id"] == session
        publish_text(saved, "")
        if scenario().get("input_gate"):
            release = Path(scenario()["input_gate"])
            publish_text(home / "input-waiting", str(os.getpid()))
            wait_for_gate(release, timeout=10, timeout_error=RuntimeError("input gate timed out"))
        emit(
            {
                "type": "system",
                "subtype": "init",
                "session_id": session,
                "model": native_model,
                "permissionMode": "default",
                "capabilities": [
                    "interrupt_receipt_v1",
                    "interrupt_cancel_queued_v1",
                    "msg_lifecycle_v1",
                ],
            }
        )
        emit(
            {
                "type": "user",
                "uuid": turn,
                "session_id": session,
                "parent_tool_use_id": None,
                "isReplay": True,
            }
        )
        content = frame["message"]["content"]
        text = content if isinstance(content, str) else "\n".join(block["text"] for block in content if block["type"] == "text")
        body = text.split("\n", 1)[1]
        append_line(home / "inputs.jsonl", json.dumps({"thread": session, "body": body, "input": text, "native_input": content, "at": time.time()}) + "\n", max_record_bytes=INPUT_RECORD_LIMIT)
        if body.startswith("Hive branch identity:"):
            assert fork, "branch initial input must belong to native --fork-session"
            assert f"You are agent {agent}," in body
            body = body.split("\n\nNew task:\n", 1)[1]
        history.append(body)
        publish_text(history_file, json.dumps(history))
        body = body.split("\n\nHive attachment references", 1)[0]
        parent = native_history[-1]["uuid"] if native_history else None
        if fork and scenario().get("branch_disconnected_history"):
            parent = None
        native_history.extend([
            {"uuid": turn, "parentUuid": parent,
             "type":"user", "sessionId":session, "message":{"role":"user","content":body}},
            {"uuid":str(uuid.uuid4()), "parentUuid":turn, "type":"assistant", "sessionId":session,
             "message":{"role":"assistant","content":[{"type":"text","text":"OK"}]}}
        ])
        publish_text(native_history_file, "".join(json.dumps(row) + "\n" for row in native_history))

        if scenario().get("quota_once") and not (home / "quota-seen").exists():
            publish_text(home / "quota-seen", "1")
            emit(
                {
                    "type": "rate_limit_event",
                    "session_id": session,
                    "rate_limit_info": {
                        "status": "rejected",
                        "resetsAt": int(time.time()) + 3600,
                        "rateLimitType": "five_hour",
                    },
                }
            )
            emit(
                {
                    "type": "assistant",
                    "session_id": session,
                    "error": "rate_limit",
                    "message": {"model": native_model, "content": []},
                }
            )
            emit(
                {
                    "type": "result",
                    "session_id": session,
                    "subtype": "error_during_execution",
                    "is_error": True,
                    "user_message_uuid": turn,
                    "num_turns": 1,
                }
            )
            turn = None
            continue
        if scenario().get("usage_info"):
            emit(
                {
                    "type": "rate_limit_event",
                    "session_id": session,
                    "rate_limit_info": scenario()["usage_info"],
                }
            )
        if body == "skill:fork":
            # Record how Hive launched this session, then call a skill that
            # forks, as Claude Code 2.1.287 runs code-review.
            publish_text(
                home / "skill-launch.json",
                json.dumps(
                    {
                        "disable_slash_commands": "--disable-slash-commands" in args,
                        "tools": option("--tools"),
                    }
                ),
            )
            skill_input = {"skill": "code-review"}
            emit(
                {
                    "type": "assistant",
                    "session_id": session,
                    "parent_tool_use_id": None,
                    "message": {
                        "model": native_model,
                        "content": [
                            {"type": "tool_use", "id": "skill-tool", "name": "Skill", "input": skill_input}
                        ],
                    },
                }
            )
            pending["skill-hook"] = "skill"
            emit(
                {
                    "type": "control_request",
                    "request_id": "skill-hook",
                    "request": {
                        "subtype": "hook_callback",
                        "callback_id": "hive-guard",
                        "input": {
                            "hook_event_name": "PreToolUse",
                            "session_id": session,
                            "tool_use_id": "skill-tool",
                            "tool_name": "Skill",
                            "tool_input": skill_input,
                        },
                    },
                }
            )
            continue
        if body in (
            "shell-guard",
            "shell-sigchld",
            "shell-stray",
            "shell-killed",
            "shell-interrupt",
            "shell-retained",
        ):
            pending["shell-hook"] = "shell"
            shell_body = 'sleep 60 & child=$!; kill -0 "$child" && echo "$child" > background.pid'
            if body in ("shell-killed", "shell-interrupt", "shell-retained"):
                shell_body = 'echo "$PPID" > helper.pid; echo "$$" > leader.pid; exec sleep 60'
            if body == "shell-stray":
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stray:
                    stray.connect(os.environ["HIVE_SHELL_GUARD_SOCKET"])
                    try:
                        stray.sendall(b"not a registered helper\n")
                    except (BrokenPipeError, ConnectionResetError):
                        # Peer authentication may reject this socket before its
                        # malformed payload is written. Both timings are valid.
                        pass
            emit(
                {
                    "type": "assistant",
                    "session_id": session,
                    "parent_tool_use_id": None,
                    "message": {
                        "model": native_model,
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "shell-tool",
                                "name": "Bash",
                                "input": {"command": shell_body},
                            }
                        ],
                    },
                }
            )
            emit(
                {
                    "type": "control_request",
                    "request_id": "shell-hook",
                    "request": {
                        "subtype": "hook_callback",
                        "callback_id": "hive-guard",
                        "input": {
                            "hook_event_name": "PreToolUse",
                            "session_id": session,
                            "tool_use_id": "shell-tool",
                            "tool_name": "Bash",
                            "tool_input": {"command": shell_body},
                        },
                    },
                }
            )
            continue
        if body in ("git-status", "git-denied"):
            # A read-only agent asks for one git command; Hive checks and renders it.
            pending["shell-hook"] = "git"
            git_command = "git status --porcelain"
            if body == "git-denied":
                git_command = "git status; touch pwned"
            emit(
                {
                    "type": "assistant",
                    "session_id": session,
                    "parent_tool_use_id": None,
                    "message": {
                        "model": native_model,
                        "content": [
                            {
                                "type": "tool_use",
                                "id": "shell-tool",
                                "name": "Bash",
                                "input": {"command": git_command},
                            }
                        ],
                    },
                }
            )
            emit(
                {
                    "type": "control_request",
                    "request_id": "shell-hook",
                    "request": {
                        "subtype": "hook_callback",
                        "callback_id": "hive-guard",
                        "input": {
                            "hook_event_name": "PreToolUse",
                            "session_id": session,
                            "tool_use_id": "shell-tool",
                            "tool_name": "Bash",
                            "tool_input": {"command": git_command},
                        },
                    },
                }
            )
            continue
        if body.startswith("backpressure:"):
            assert body == "backpressure:" + "x" * 65523
            complete("large-input-intact")
            continue
        if body in ("api-error", "api-error-exit"):
            emit(
                {
                    "type": "result",
                    "session_id": session,
                    "subtype": "success",
                    "is_error": True,
                    "api_error_status": 529,
                    "terminal_reason": "completed",
                }
            )
            turn = None
            # Native runHeadless exits1 after a terminal is_error result on EOF.
            if body == "api-error":
                sys.stdin.read()
            raise SystemExit(1)
        if body in ("hold-error-interrupt", "hold-error-interrupt-exit"):
            mode = body.removeprefix("hold-")
            continue
        if body == "hold":
            continue
        if body in ("reply:split", "reply:split-large"):
            fragments = (
                ["x" * 4096] * 8 if body.endswith("-large") else ["first block", "second block"]
            )
            for text in fragments:
                emit(
                    {
                        "type": "assistant",
                        "session_id": session,
                        "message": {
                            "id": "native-" + turn,
                            "model": native_model,
                            "content": [{"type": "text", "text": text}],
                        },
                    }
                )
            finish()
            continue
        if body.startswith("mcp:"):
            name = body[4:]
            assert f"mcp__{name}__*" in option("--allowedTools")
            complete(run_stdio_mcp(json.loads(option("--mcp-config"))["mcpServers"][name]))
        elif body.startswith("comb:"):
            comb_actions = json.loads(body[len("comb:"):])
            comb_results = []
            comb_next()
        elif body.startswith("joined:"):
            tool("hive_batch", json.loads(body[7:]), "asked")
        elif body.startswith("join-output:"):
            character = {"ascii": "x", "control": "\x01", "utf8": "🙂"}[body.split(":")[1]]
            complete(character * (65536 // len(character.encode())))
        elif body.startswith("Joined results for "):
            complete("joined reply consumed")
        elif body == "branch self":
            tool(
                "hive_spawn",
                {"from_agent": agent, "body": "branch recall", "request_id": "branch-self"},
                "spawned",
            )
        elif body == "branch recall":
            complete(json.dumps({"history": history, "agent": agent}))
        elif body.startswith("ask:"):
            tool("hive_ask", json.loads(body[4:]), "asked")
        elif body == "delay-self":
            tool(
                "hive_send",
                {
                    "target": agent,
                    "body": "reply:reminder",
                    "delay_seconds": 2,
                    "request_id": "self-reminder",
                },
                "scheduled",
            )
        elif body == "spawn bound child":
            tool(
                "hive_spawn",
                {
                    "role": "claude",
                    "lifetime": "with_parent",
                    "body": "hold",
                    "request_id": "bound-child",
                },
                "spawned",
            )
        elif body == "spawn-reviewer":
            tool(
                "hive_spawn",
                {"role": "claude-reviewer", "body": "review-one", "request_id": "spawn-" + agent},
                "spawned",
            )
        elif body == "review-one":
            parent = (
                Path(option("--append-system-prompt-file"))
                .read_text()
                .split("parent ", 1)[1]
                .split(".", 1)[0]
            )
            tool(
                "hive_send",
                {"target": parent, "body": "review-ok", "request_id": "review-" + agent},
                "sent",
            )
        elif body == "review-ok":
            tool(
                "hive_complete", {"body": "reviewed", "request_id": "complete-" + agent}, "sent"
            )
        else:
            complete("answer:" + body)
    elif kind == "control_response":
        if frame["response"]["subtype"] == "success":
            emit(frame)  # native only echoes tracked successful control replies
        response = frame["response"]
        continuation = pending.pop(response["request_id"])
        if continuation == "backpressure":
            assert response["response"]["mcp_response"]["id"] == 42
            assert response["response"]["mcp_response"]["result"] == {}
            publish_text(home / "backpressure-confirmed", "")
            continue
        if continuation == "skill":
            if response["response"]["hookSpecificOutput"]["permissionDecision"] != "allow":
                emit(
                    {
                        "type": "user",
                        "session_id": session,
                        "message": {
                            "content": [
                                {
                                    "type": "tool_result",
                                    "tool_use_id": "skill-tool",
                                    "content": "denied",
                                    "is_error": True,
                                }
                            ]
                        },
                    }
                )
                complete("skill-denied")
                continue
            emit(
                {
                    "type": "system",
                    "subtype": "task_started",
                    "session_id": session,
                    "task_id": "skill-task",
                    "tool_use_id": "skill-tool",
                    "description": "/code-review",
                    "is_backgrounded": False,
                    "task_type": "local_agent",
                }
            )
            emit(
                {
                    "type": "user",
                    "session_id": session,
                    "parent_tool_use_id": "skill-tool",
                    "isSynthetic": True,
                    "message": {"content": [{"type": "text", "text": "Review the diff."}]},
                }
            )
            read_input = {"file_path": str(Path.cwd() / "reviewed.txt")}
            emit(
                {
                    "type": "assistant",
                    "session_id": session,
                    "parent_tool_use_id": "skill-tool",
                    "message": {
                        "model": native_model,
                        "content": [
                            {"type": "tool_use", "id": "skill-read", "name": "Read", "input": read_input}
                        ],
                    },
                }
            )
            pending["skill-read-hook"] = "skill-read"
            emit(
                {
                    "type": "control_request",
                    "request_id": "skill-read-hook",
                    "request": {
                        "subtype": "hook_callback",
                        "callback_id": "hive-guard",
                        "input": {
                            "hook_event_name": "PreToolUse",
                            "session_id": session,
                            "tool_use_id": "skill-read",
                            "tool_name": "Read",
                            "tool_input": read_input,
                        },
                    },
                }
            )
            continue
        if continuation == "skill-read":
            assert response["response"]["hookSpecificOutput"]["permissionDecision"] == "allow"
            emit(
                {
                    "type": "user",
                    "session_id": session,
                    "parent_tool_use_id": "skill-tool",
                    "message": {
                        "content": [{"type": "tool_result", "tool_use_id": "skill-read", "content": "read"}]
                    },
                }
            )
            emit(
                {
                    "type": "tool_progress",
                    "session_id": session,
                    "parent_tool_use_id": "skill-tool",
                    "tool_use_id": "skill-tool-heartbeat-0",
                    "tool_name": "Skill",
                    "heartbeat": True,
                }
            )
            emit(
                {
                    "type": "system",
                    "subtype": "task_notification",
                    "session_id": session,
                    "task_id": "skill-task",
                    "tool_use_id": "skill-tool",
                    "status": "completed",
                }
            )
            emit(
                {
                    "type": "user",
                    "session_id": session,
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "skill-tool",
                                "content": 'Skill "code-review" completed (forked execution).',
                            }
                        ]
                    },
                }
            )
            complete("skill-forked")
            continue
        if continuation == "git":
            hook = response["response"]["hookSpecificOutput"]
            if hook["permissionDecision"] != "allow":
                result = "denied: " + hook["permissionDecisionReason"]
            else:
                command = hook["updatedInput"]["command"]
                shell = subprocess.run(
                    [
                        os.environ["CLAUDE_CODE_SHELL"],
                        "-c",
                        "exec " + shlex.join([os.environ["CLAUDE_CODE_SHELL_PREFIX"], command]),
                    ],
                    capture_output=True,
                    check=False,
                )
                result = "exit " + str(shell.returncode) + ": " + shell.stdout.decode().strip()
            emit(
                {
                    "type": "user",
                    "session_id": session,
                    "parent_tool_use_id": None,
                    "message": {
                        "content": [
                            {"type": "tool_result", "tool_use_id": "shell-tool", "content": result}
                        ]
                    },
                }
            )
            complete("git " + result)
            continue
        if continuation == "shell":
            hook = response["response"]["hookSpecificOutput"]
            assert hook["permissionDecision"] == "allow", hook
            command = hook["updatedInput"]["command"]
            shell_args = [
                os.environ["CLAUDE_CODE_SHELL"],
                "-c",
                "exec " + shlex.join([os.environ["CLAUDE_CODE_SHELL_PREFIX"], command]),
            ]
            if body in ("shell-killed", "shell-interrupt", "shell-retained"):
                phase = "start-helper"
                try:
                    shell = subprocess.Popen(
                        shell_args, stdout=subprocess.PIPE, stderr=subprocess.PIPE
                    )
                    deadline = time.monotonic() + 15
                    while True:
                        try:
                            helper = int(Path("helper.pid").read_text())
                            leader = int(Path("leader.pid").read_text())
                            break
                        except (FileNotFoundError, ValueError):
                            if shell.poll() is not None:
                                stdout, stderr = shell.communicate()
                                publish_text(Path("shell-diagnostic"), str(shell.returncode) + ":" + stderr.decode())
                                raise AssertionError("helper exited before command started")
                            assert time.monotonic() < deadline
                            time.sleep(0.01)
                    if body == "shell-interrupt":
                        shell_process = shell
                        emit(
                            {
                                "type": "assistant",
                                "session_id": session,
                                "parent_tool_use_id": None,
                                "message": {
                                    "model": native_model,
                                    "content": [{"type": "text", "text": "shell-running"}],
                                },
                            }
                        )
                        continue
                    phase = "kill-helper"
                    os.kill(helper, signal.SIGKILL)
                    if body == "shell-retained":
                        # Hold our known-live test group past the guard's 3s proof
                        # deadline, then explicitly clean it even if daemon stops.
                        # Uncertain Claude turns now receive TERM immediately; keep
                        # this fixture alive long enough to reap its retained group.
                        signal.signal(signal.SIGTERM, signal.SIG_IGN)
                        emit(
                            {
                                "type": "user",
                                "session_id": session,
                                "message": {
                                    "content": [
                                        {
                                            "type": "tool_result",
                                            "tool_use_id": "shell-tool",
                                            "content": "native returned",
                                        }
                                    ]
                                },
                            }
                        )
                        finish()
                        time.sleep(4)
                    phase = "kill-group"
                    os.killpg(leader, signal.SIGKILL)
                    phase = "reap-helper"
                    stdout, stderr = shell.communicate(timeout=5)
                    publish_text(Path("shell-diagnostic"), json.dumps(
                            {
                                "phase": "reaped",
                                "helper": helper,
                                "leader": leader,
                                "exit": shell.returncode,
                                "stderr": stderr.decode(),
                            }
                        ))
                    assert shell.returncode != 0
                    if body == "shell-retained":
                        publish_text(Path("retained-cleaned"), "")
                        signal.signal(signal.SIGTERM, signal.SIG_DFL)
                        continue
                except BaseException as error:
                    publish_text(Path("shell-diagnostic"), json.dumps({"phase": phase, "error": repr(error)}))
                    raise

            else:
                if body == "shell-sigchld":
                    signal.signal(signal.SIGCHLD, signal.SIG_IGN)
                shell = subprocess.run(shell_args, capture_output=True, check=False)
                if body == "shell-sigchld":
                    signal.signal(signal.SIGCHLD, signal.SIG_DFL)
                publish_text(Path("shell-diagnostic"), str(shell.returncode) + ":" + shell.stderr.decode())
                assert shell.returncode == 0, (shell.returncode, shell.stderr)
            emit(
                {
                    "type": "user",
                    "session_id": session,
                    "parent_tool_use_id": None,
                    "message": {
                        "content": [
                            {"type": "tool_result", "tool_use_id": "shell-tool", "content": "done"}
                        ]
                    },
                }
            )
            complete("shell-clean")
            continue
        result = response["response"]["mcp_response"]["result"]
        if continuation == "comb-result":
            comb_results.append(json.loads(result["content"][0]["text"]))
            comb_next()
            continue
        assert not result["isError"], result
        if continuation == "spawned":
            child = json.loads(result["content"][0]["text"])["agent_id"]
            complete("reviewer:" + child)
        else:
            finish()
    else:
        raise AssertionError(frame)
