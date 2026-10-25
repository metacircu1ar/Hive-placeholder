import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import threading
import time
import uuid
from dataclasses import dataclass, field

from mock_common import run_stdio_mcp, load_scenario, wait_for_gate, publish_text, append_line, claim_counter
from mock_common import INPUT_RECORD_LIMIT

from mock_common import schema_probe
if schema_probe(sys.argv):
    raise SystemExit()

home = pathlib.Path(os.environ["CODEX_HOME"])
scenario = load_scenario(home / "runtime-scenario.json")
if "--version" in sys.argv:
    version = scenario.get("provider_version", "0.163.0")
    print("codex-cli " + version)
    raise SystemExit()
append_line(home / "process-options.jsonl", json.dumps({
    "pid": os.getpid(),
    "compression": [arg for arg in sys.argv if arg.startswith("features.enable_request_compression=")],
}) + "\n")
if scenario.get("require_sigterm"):
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
if scenario.get("close_fds"):
    os.closerange(3, 1024)
if scenario.get("pid_file"):
    publish_text(pathlib.Path(scenario["pid_file"]), str(os.getpid()))


@dataclass
class Conversation:
    thread_id: str | None = None
    turn_id: str | None = None
    active_turn: bool = False
    meta: dict = field(default_factory=dict)
    pending: str | None = None
    pending_args: dict | None = None
    steering_request: dict | None = None
    actions: list = field(default_factory=list)
    command_child: subprocess.Popen | None = None
    list_calls: int = 0


conversations = {}
callbacks = {}
loaded_threads = set()


def select_conversation(request, conversation):
    # Commands from different resident conversations and tool replies may interleave.
    if conversation.thread_id is not None:
        conversations[conversation.thread_id] = conversation
    method = request.get("method")
    params = request.get("params", {})
    if method in ("thread/start", "thread/fork"):
        return Conversation()
    if method == "thread/resume":
        return Conversation(thread_id=params["threadId"])
    if "threadId" in params:
        return conversations[params["threadId"]]
    if method is None and "result" in request:
        return conversations[callbacks.pop(request["id"])]
    return conversation


def emit(value):
    print(json.dumps(value), flush=True)


def reply(request, result):
    emit({"id": request["id"], "result": result})


def gated_ack(request, turn, gate):
    if wait_for_gate(gate, timeout=10, strict_deadline=True):
        reply(request, {"turnId": turn})


def save(conversation):
    publish_text(home / (conversation.thread_id + ".json"), json.dumps(conversation.meta))


def finish(conversation, status="completed"):
    conversation.active_turn = False
    conversation.meta["last_turn"] = conversation.turn_id
    save(conversation)
    emit(
        {
            "method": "turn/completed",
            "params": {
                "threadId": conversation.thread_id,
                "turn": {"id": conversation.turn_id, "status": status},
            },
        }
    )


def late_terminal(conversation):
    assert conversation.command_child is None
    conversation.command_child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(120)"]
    )
    publish_text(home / ("late-shell-" + conversation.meta["agent"]), str(conversation.command_child.pid))
    emit(
        {
            "method": "item/started",
            "params": {
                "threadId": conversation.thread_id,
                "turnId": conversation.turn_id,
                "item": {"id": "late-command", "type": "commandExecution"},
            },
        }
    )


def call(conversation, name, args, native_id=None):
    conversation.pending = name
    conversation.pending_args = args
    tool_id = "tool-" + conversation.turn_id if native_id is None else "comb-" + str(native_id)
    callbacks[tool_id] = conversation.thread_id
    emit(
        {
            "id": tool_id,
            "method": "item/tool/call",
            "params": {
                "threadId": conversation.thread_id,
                "turnId": conversation.turn_id,
                "callId": "call-" + tool_id,
                "tool": name,
                "arguments": args,
            },
        }
    )


def handle_tool_result(request, conversation):
    result = request["result"]
    if conversation.meta.pop("tool_reply", False):
        conversation.pending = None
        text = result["contentItems"][0]["text"]
        body_reply(conversation, json.dumps({"success": result["success"], "text": text}), None)
        return
    if conversation.meta.get("comb_script"):
        conversation.meta["comb_results"].append(json.loads(result["contentItems"][0]["text"]))
        comb_next(conversation)
        return
    if conversation.pending == "hive_status" and conversation.steering_request is not None:
        assert result["success"], result
        reply(conversation.steering_request, {"turnId": conversation.turn_id})
        conversation.steering_request = None
        conversation.pending = None
        finish(conversation)
        return
    if conversation.meta.get("ping_pong"):
        assert result["success"], result
        value = json.loads(result["contentItems"][0]["text"])
        if conversation.pending == "hive_spawn":
            conversation.meta.setdefault("children", []).append(value["agent_id"])
            save(conversation)
        emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {
                        "type": "dynamicToolCall",
                        "tool": conversation.pending,
                        "arguments": conversation.pending_args,
                        "status": "completed",
                        "success": True,
                        "contentItems": result["contentItems"],
                    },
                },
            }
        )
        conversation.pending = None
        if conversation.actions:
            call(conversation, *conversation.actions.pop(0))
        else:
            finish(conversation)
        return
    if conversation.pending == "hive_send" and conversation.pending_args.get("delay_seconds"):
        assert result["success"], result
    if conversation.pending == "hive_batch":
        assert result["success"], result
    if conversation.pending == "hive_ask":
        assert result["success"], result
        emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {
                        "type": "dynamicToolCall",
                        "tool": conversation.pending,
                        "arguments": conversation.pending_args,
                        "status": "completed",
                        "success": True,
                        "contentItems": result["contentItems"],
                    },
                },
            }
        )
    if conversation.pending == "hive_spawn":
        conversation.meta["reviewer"] = json.loads(result["contentItems"][0]["text"])["agent_id"]
        save(conversation)
    if conversation.pending != "input_required" and not (
        conversation.pending == "hive_send" and conversation.pending_args.get("mode") == "steer"
    ):
        finish(conversation)
    conversation.pending = None


def handle_initialize(request, conversation):
    if scenario.get("record_initialize"):
        append_line(home / "initialize.jsonl", json.dumps(request.get("params", {})) + "\n")
    reply(request, {})


def stream_live(conversation, item_id, text):
    # Opt-in live mode: a command with output, a reasoning summary, then the
    # answer as small deltas, as native Codex streams them before item/completed.
    base = {"threadId": conversation.thread_id, "turnId": conversation.turn_id}
    delay = scenario.get("delta_delay", 0)
    command = {"type": "commandExecution", "id": "cmd-" + conversation.turn_id,
               "command": "echo live", "cwd": ".", "commandActions": [], "status": "inProgress"}
    emit({"method": "item/started", "params": {**base, "item": command}})
    emit({"method": "item/commandExecution/outputDelta",
          "params": {**base, "itemId": command["id"], "delta": "live\n"}})
    emit({"method": "item/completed",
          "params": {**base, "item": {**command, "status": "completed", "exitCode": 0}}})
    emit({"method": "item/reasoning/summaryTextDelta",
          "params": {**base, "itemId": "rs-" + conversation.turn_id, "summaryIndex": 0,
                     "delta": "Planning the answer."}})
    # Raw reasoning is opted out; a provider that sends it anyway must be ignored.
    emit({"method": "item/reasoning/textDelta",
          "params": {**base, "itemId": "rs-" + conversation.turn_id, "contentIndex": 0,
                     "delta": "raw"}})
    emit({"method": "item/started", "params": {**base, "item": {
        "type": "userMessage", "id": "echo-" + conversation.turn_id,
        "content": [{"type": "text", "text": "echoed input"}]}}})
    chunk = scenario.get("delta_chars", 3)
    for start in range(0, len(text), chunk):
        emit({"method": "item/agentMessage/delta",
              "params": {**base, "itemId": item_id, "delta": text[start:start + chunk]}})
        if delay:
            time.sleep(delay)


def handle_account(request, conversation):
    kind = scenario.get("api_account", "apiKey" if api_key is not None else "chatgpt")
    reply(request, {"requiresOpenaiAuth": True, "account": {"type": kind}})


api_key = None


def handle_requirements(request, conversation):
    if scenario.get("api_requirements_unsupported"):
        emit({"id": request["id"], "error": {"code": -32601, "message": "unsupported"}})
    else:
        reply(request, scenario.get("api_requirements", {"requirements": None}))


def handle_config(request, conversation):
    reply(request, scenario.get("api_config", {"config": {
        "cli_auth_credentials_store": "ephemeral", "model_provider": "openai"}}))


def handle_api_login(request, conversation):
    global api_key
    assert scenario.get("api_key"), "subscription must not perform API login"
    stores = [value for value in sys.argv if value.startswith("cli_auth_credentials_store=")]
    assert stores[-1] == 'cli_auth_credentials_store="ephemeral"'
    assert "OPENAI_API_KEY" not in os.environ and "CODEX_API_KEY" not in os.environ
    assert "ANTHROPIC_API_KEY" not in os.environ
    api_key = request["params"]["apiKey"]
    assert api_key and api_key not in sys.argv
    emit({"method": "account/updated", "params": {"private": api_key}})
    if scenario.get("api_login_error"):
        emit({"id": request["id"], "error": {"code": -32000, "message": api_key, "data": api_key}})
    else:
        if scenario.get("api_auth_file"):
            publish_text(home / "auth.json", api_key)
        reply(request, {"type": scenario.get("api_login_kind", "apiKey")})


def handle_rate_limits(request, conversation):
    reply(request, {"rateLimits": scenario.get("rate_limits_read", {}), "ordinaryUsageAllowed": None})


def handle_models(request, conversation):
    if scenario.get("model_list_gate"):
        release = pathlib.Path(scenario["model_list_gate"])
        publish_text(pathlib.Path(scenario["model_list_waiting"]), str(os.getpid()))
        wait_for_gate(release, timeout=30, timeout_error=RuntimeError("catalog gate timed out"))
    if scenario.get("model_list_delay"):
        time.sleep(scenario["model_list_delay"])
    models = scenario.get("catalog_models", ["gpt-5.6-terra", "gpt-5.6-sol", "gpt-5.6-luna"])
    if scenario.get("catalog_once_model") and claim_counter(home / "catalog-once", 1) is not None:
        models = models + [scenario["catalog_once_model"]]
    reply(
        request,
        {
            "data": [
                {
                    "model": m,
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": e} for e in ("high", "medium")
                    ],
                }
                for m in models
            ],
            "nextCursor": None,
        },
    )


def handle_thread(request, conversation):
    method = request.get("method")
    p = request.get("params", {})
    if method == "thread/fork":
        source = json.loads((home / (p["threadId"] + ".json")).read_text())
        assert p["lastTurnId"] == source["last_turn"], (p, source)
        assert p["excludeTurns"] is True and p["ephemeral"] is False
        rejections = scenario.get("fork_rejections", [])
        rejected = claim_counter(home / "fork-rejected", len(rejections))
        if rejected is not None:
            emit(
                {
                    "id": request["id"],
                    "error": {
                        "code": -32600,
                        "message": rejections[rejected].format(turn=p["lastTurnId"]),
                    },
                }
            )
            return
        conversation.thread_id = str(uuid.uuid4())
        conversation.meta = source
        conversation.meta["forked_from"] = p["threadId"]
        conversation.meta["agent"] = re.search(
            r"You are Hive agent ([^,]+)", p["developerInstructions"]
        ).group(1)
        conversation.meta["parent"] = re.search(
            r"parent ([^. ]+)", p["developerInstructions"]
        ).group(1)
        conversation.meta["instructions"] = p["developerInstructions"]
        conversation.meta["model"], conversation.meta["effort"] = (
            p["model"],
            p["config"]["model_reasoning_effort"],
        )
        save(conversation)
        if scenario.get("fork_hold_after_creation"):
            publish_text(home / "fork-created", conversation.thread_id)
            wait_for_gate(home / "fork-release")
    if method == "thread/resume" and scenario.get("reject_resume"):
        emit({"id": request["id"], "error": {"code": -1, "message": "resume rejected"}})
        return
    if method == "thread/start":
        conversation.thread_id = str(uuid.uuid4())
        parent = re.search(r"parent ([^. ]+)", p["developerInstructions"]).group(1)
        agent = re.search(r"You are Hive agent ([^,]+)", p["developerInstructions"]).group(1)
        conversation.meta = {
            "instructions": p["developerInstructions"],
            "parent": parent,
            "agent": agent,
            "model": p["model"],
            "effort": p["config"]["model_reasoning_effort"],
            "tools": p["dynamicTools"],
        }
        save(conversation)
    elif method == "thread/resume":
        conversation.thread_id = p["threadId"]
        conversation.meta = json.loads((home / (conversation.thread_id + ".json")).read_text())
        assert "dynamicTools" not in p
        assert conversation.meta["tools"]
    conversation.meta["mcp_servers"] = p["config"]["mcp_servers"]
    loaded_threads.add(conversation.thread_id)
    append_line(home / "thread-selections.jsonl", json.dumps({
        "method": method,
        "thread": conversation.thread_id,
        "pid": os.getpid(),
        "model": p["model"],
        "config": p["config"],
    }) + "\n")
    save(conversation)
    retained = method == "thread/resume" and scenario.get("resume_ignores_selection")
    if method == "thread/resume" and scenario.get("resume_echo_model"):
        conversation.meta["model"] = scenario["resume_echo_model"]
    if method == "thread/resume" and scenario.get("resume_echo_effort"):
        conversation.meta["effort"] = scenario["resume_echo_effort"]
    reply(
        request,
        {
            "thread": {"id": conversation.thread_id},
            "model": conversation.meta["model"] if retained else p["model"],
            "reasoningEffort": (
                conversation.meta["effort"] if retained else p["config"]["model_reasoning_effort"]
            ),
            "sandbox": {"type": "workspaceWrite"},
        },
    )


def handle_turn_start(request, conversation):
    p = request.get("params", {})
    if scenario.get("reject_turn"):
        if scenario.get("rejected_but_started"):
            emit(
                {
                    "method": "turn/started",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turn": {"id": "unexpected", "status": "inProgress"},
                    },
                }
            )
        emit({"id": request["id"], "error": {"code": -1, "message": "turn rejected"}})
        return
    append_line(home / "turn-selections.jsonl", json.dumps(
                {"thread": conversation.thread_id, "model": p["model"], "effort": p["effort"],
                 "summary": p.get("summary")}
            )
            + "\n")
    if scenario.get("persist_turn_selection"):
        conversation.meta["model"], conversation.meta["effort"] = p["model"], p["effort"]
        save(conversation)
    if scenario.get("turn_start_gate"):
        gate = pathlib.Path(scenario["turn_start_gate"])
        wait_for_gate(gate, timeout=10, timeout_error=RuntimeError("turn start gate timed out"))
    conversation.turn_id = str(uuid.uuid4())
    conversation.active_turn = True
    if scenario.get("missing_turn_ack"):
        reply(request, {"turn": {}})
        return
    reply(request, {"turn": {"id": conversation.turn_id}})
    emit(
        {
            "method": "turn/started",
            "params": {
                "threadId": conversation.thread_id,
                "turn": {"id": conversation.turn_id, "status": "inProgress"},
            },
        }
    )
    body, sender = record_input(conversation, p)
    if scenario.get("echo_input_items"):
        item = {"id": str(uuid.uuid4()), "type": "userMessage", "content": p["input"]}
        for method in ("item/started", "item/completed"):
            emit({"method": method, "params": {
                "threadId": conversation.thread_id,
                "turnId": conversation.turn_id,
                "item": item,
            }})
    if quota_error(conversation, body):
        return
    if scenario.get("rate_limits_update"):
        # A sparse rolling update, as Codex sends during turns.
        emit({"method": "account/rateLimits/updated",
              "params": {"rateLimits": scenario["rate_limits_update"]}})
    handle_body(conversation, body, sender)


def body_branch_self(conversation, body, sender):
    call(
        conversation,
        "hive_spawn",
        {
            "from_agent": conversation.meta["agent"],
            "body": "branch recall",
            "request_id": "branch-self",
        },
    )


def body_branch_recall(conversation, body, sender):
    emit(
        {
            "method": "item/completed",
            "params": {
                "threadId": conversation.thread_id,
                "turnId": conversation.turn_id,
                "item": {
                    "type": "agentMessage",
                    "text": json.dumps(
                        {
                            "history": conversation.meta["history"],
                            "agent": conversation.meta["agent"],
                        }
                    ),
                    "phase": "final_answer",
                },
            },
        }
    )
    finish(conversation)


def body_ask(conversation, body, sender):
    call(conversation, "hive_ask", json.loads(body[4:]))


def body_spawn_bound_child(conversation, body, sender):
    call(
        conversation,
        "hive_spawn",
        {
            "role": "ping",
            "lifetime": "with_parent",
            "body": "hold",
            "request_id": "bound-child",
        },
    )


def body_spawn_mixed_children(conversation, body, sender):
    conversation.meta["ping_pong"] = True
    conversation.actions = [
        ("hive_spawn", {"role": role, "body": "idle", "request_id": role})
        for role in ("reviewer", "ping")
    ]
    call(conversation, *conversation.actions.pop(0))


def body_ping_pong(conversation, body, sender):
    conversation.meta["ping_pong"] = True
    conversation.actions = [
        (
            "hive_spawn",
            {
                "role": scenario.get("ping_role", "ping"),
                "body": "Send ping to your parent now.",
                "request_id": f"ping-{i:02}",
            },
        )
        for i in range(1, scenario.get("ping_count", 10) + 1)
    ]
    call(conversation, *conversation.actions.pop(0))


def body_initial_ping(conversation, body, sender):
    conversation.meta["ping_pong"] = True
    call(
        conversation,
        "hive_send",
        {"target": conversation.meta["parent"], "body": "ping", "request_id": "initial-ping"},
    )


def body_ping(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {"target": sender, "body": "pong", "request_id": "pong-" + sender},
    )


def body_pong(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {
            "target": conversation.meta["parent"],
            "body": "received pong",
            "request_id": "pong-receipt",
        },
    )


def body_pong_receipt(conversation, body, sender):
    conversation.meta.setdefault("receipts", []).append(sender)
    save(conversation)
    if len(set(conversation.meta["receipts"])) == scenario.get("ping_count", 10):
        conversation.actions = [
            ("hive_terminate", {"target": child, "request_id": "terminate-" + child})
            for child in conversation.meta["children"]
        ]
        conversation.actions.append(
            (
                "hive_complete",
                {"body": "10 pings, pongs and receipts", "request_id": "ping-pong-complete"},
            )
        )
        call(conversation, *conversation.actions.pop(0))
    else:
        finish(conversation)


def body_delegate(conversation, body, sender):
    call(
        conversation,
        "hive_spawn",
        {"role": "reviewer", "body": "review one", "request_id": "spawn-reviewer"},
    )


def body_delay_self(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {
            "target": conversation.meta["agent"],
            "body": "reply:reminder",
            "delay_seconds": 2,
            "request_id": "self-reminder",
        },
    )


def body_review_one(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {"target": conversation.meta["parent"], "body": "findings", "request_id": "findings"},
    )


def body_findings(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {
            "target": conversation.meta["reviewer"],
            "body": "review two",
            "request_id": "second-review",
        },
    )


def body_review_two(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {"target": conversation.meta["parent"], "body": "NO_FINDINGS", "request_id": "clean"},
    )


def body_complete(conversation, body, sender):
    call(
        conversation,
        "hive_complete",
        {"body": "reviewed and tested", "request_id": "complete"},
    )


def body_bad_arguments(conversation, body, sender):
    call(conversation, "hive_agents", [])


def body_extra_arguments(conversation, body, sender):
    call(
        conversation,
        "hive_complete",
        {"body": "wrong", "request_id": "wrong", "target": "someone-else"},
    )


def body_hold(conversation, body, sender):
    pass


def body_shell(conversation, body, sender):
    conversation.command_child = subprocess.Popen(
        ["/bin/bash", "-c", "exec sleep 60"], start_new_session=True
    )
    publish_text(home / "shell-child", str(conversation.command_child.pid))
    publish_text(home / ("shell-child-" + conversation.meta["agent"]), str(conversation.command_child.pid))
    if body == "shell background":
        finish(conversation)


def body_steer_self(conversation, body, sender):
    call(
        conversation,
        "hive_send",
        {
            "target": conversation.meta["agent"],
            "body": "self guidance",
            "mode": "steer",
            "request_id": "self-steer",
        },
    )


def body_input_required(conversation, body, sender):
    conversation.command_child = subprocess.Popen(
        [
            sys.executable,
            "-c",
            'import pathlib,sys,time;time.sleep(3);pathlib.Path(sys.argv[1]).write_text("survived")',
            scenario["survivor"],
        ],
        start_new_session=True,
    )
    conversation.pending = "input_required"
    callback = "unsupported-" + conversation.turn_id
    callbacks[callback] = conversation.thread_id
    emit(
        {
            "id": callback,
            "method": "item/tool/requestUserInput",
            "params": {"threadId": conversation.thread_id, "turnId": conversation.turn_id},
        }
    )


def body_reply(conversation, body, sender):
    item = {"type": "agentMessage", "text": body, "phase": "final_answer"}
    if scenario.get("live_deltas"):
        item["id"] = "msg-" + conversation.turn_id
        stream_live(conversation, item["id"], body)
    emit(
        {
            "method": "item/completed",
            "params": {
                "threadId": conversation.thread_id,
                "turnId": conversation.turn_id,
                "item": item,
            },
        }
    )
    finish(conversation)


def body_fail(conversation, body, sender):
    finish(conversation, "failed")


def comb_next(conversation):
    if conversation.actions:
        action = conversation.actions.pop(0)
        sequence = len(conversation.meta["comb_results"])
        call(conversation, action["name"], action.get("arguments", {}), action.get("native_id", sequence))
    else:
        conversation.meta["comb_script"] = False
        body_reply(conversation, "comb-results:" + json.dumps(conversation.meta["comb_results"]), "human")


def handle_body(conversation, body, sender):
    if body.startswith("mcp:"):
        body_reply(conversation, run_stdio_mcp(conversation.meta["mcp_servers"][body[4:]]), sender)
    elif body.startswith("comb:"):
        conversation.meta["comb_script"] = True
        conversation.meta["comb_results"] = []
        conversation.actions = json.loads(body[len("comb:"):])
        comb_next(conversation)
    elif body.startswith("joined:"):
        call(conversation, "hive_batch", json.loads(body[7:]))
    elif body.startswith("tool:"):
        # One hosted tool call; its result becomes the final answer.
        request = json.loads(body[5:])
        conversation.meta["tool_reply"] = True
        call(conversation, request["name"], request["arguments"])
    elif body.startswith("join-output:"):
        character = {"ascii": "x", "control": "\x01", "utf8": "🙂"}[body.split(":")[1]]
        body_reply(conversation, character * (65536 // len(character.encode())), sender)
    elif body == "branch self":
        body_branch_self(conversation, body, sender)
    elif body == "branch recall":
        body_branch_recall(conversation, body, sender)
    elif body.startswith("ask:"):
        body_ask(conversation, body, sender)
    elif body == "spawn bound child":
        body_spawn_bound_child(conversation, body, sender)
    elif body == "spawn mixed children":
        body_spawn_mixed_children(conversation, body, sender)
    elif body == "ping pong scenario":
        body_ping_pong(conversation, body, sender)
    elif body == "Send ping to your parent now.":
        body_initial_ping(conversation, body, sender)
    elif body == "ping":
        body_ping(conversation, body, sender)
    elif body == "pong":
        body_pong(conversation, body, sender)
    elif body == "received pong":
        body_pong_receipt(conversation, body, sender)
    elif body == "delegate":
        body_delegate(conversation, body, sender)
    elif body == "delay-self":
        body_delay_self(conversation, body, sender)
    elif body == "review one":
        body_review_one(conversation, body, sender)
    elif body == "findings":
        body_findings(conversation, body, sender)
    elif body == "review two":
        body_review_two(conversation, body, sender)
    elif body in ["NO_FINDINGS", "complete"]:
        body_complete(conversation, body, sender)
    elif body == "bad arguments":
        body_bad_arguments(conversation, body, sender)
    elif body == "extra arguments":
        body_extra_arguments(conversation, body, sender)
    elif body == "hold":
        body_hold(conversation, body, sender)
    elif body in ("shell hold", "shell background"):
        body_shell(conversation, body, sender)
    elif body == "steer self":
        body_steer_self(conversation, body, sender)
    elif body == "input required":
        body_input_required(conversation, body, sender)
    elif body.startswith(("reply:", "Approval ")):
        body_reply(conversation, body, sender)
    elif body == "fail":
        body_fail(conversation, body, sender)
    else:
        finish(conversation)


def handle_steer(request, conversation):
    p = request.get("params", {})
    if not conversation.active_turn and scenario.get("probe_gate"):
        wait_for_gate(
            scenario["probe_gate"],
            timeout=10,
            timeout_error=AssertionError("Probe gate was never released"),
        )
    if scenario.get("empty_input_first") and not p["input"]:
        emit({"id": request["id"], "error": {"code": -32600, "message": "input must not be empty"}})
    elif scenario.get("probe_stale") and not conversation.active_turn:
        emit(
            {
                "id": request["id"],
                "error": {
                    "code": -32600,
                    "message": f"expected active turn id `{p['expectedTurnId']}` but found `untracked`",
                },
            }
        )
    elif scenario.get("probe_success") and not conversation.active_turn:
        reply(request, {"turnId": p["expectedTurnId"]})
    elif scenario.get("no_steering"):
        emit({"id": request["id"], "error": {"code": -32601, "message": "unsupported"}})
    elif not conversation.active_turn:
        message = (
            "different validation failure"
            if scenario.get("unknown_steering_probe")
            else "no active turn to steer"
        )
        emit({"id": request["id"], "error": {"code": -32600, "message": message}})
    elif p["expectedTurnId"] != conversation.turn_id:
        emit(
            {
                "id": request["id"],
                "error": {
                    "code": -32600,
                    "message": f"expected active turn id `{p['expectedTurnId']}` but found `{conversation.turn_id}`",
                },
            }
        )
    elif scenario.get("stale_steer"):
        finish(conversation)
        emit({"id": request["id"], "error": {"code": -32600, "message": "no active turn to steer"}})
    elif scenario.get("reject_steer") or scenario.get("uncertain_steer"):
        code = -32603 if scenario.get("uncertain_steer") else -32600
        emit({"id": request["id"], "error": {"code": code, "message": "steering failed"}})
    elif scenario.get("exit_steer"):
        sys.exit(0)
    elif scenario.get("steer_ack_after_tool"):
        conversation.steering_request = request
        call(conversation, "hive_status", {"target": conversation.meta["agent"]})
    elif scenario.get("steer_ack_after_interrupt"):
        conversation.steering_request = request
        publish_text(pathlib.Path(scenario["steer_ack_after_interrupt"]), "received")
    else:
        append_line(home / "steers.jsonl", json.dumps(p) + "\n", max_record_bytes=INPUT_RECORD_LIMIT)
        body = p["input"][0]["text"].split(":\n", 1)[1]
        if scenario.get("finish_before_steer_ack"):
            finish(conversation)
        reply(
            request,
            {"turnId": "wrong-turn" if scenario.get("wrong_steer_ack") else conversation.turn_id},
        )
        emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {"type": "agentMessage", "text": "steered: " + body},
                },
            }
        )
        if body in ("finish", "self guidance") and conversation.active_turn:
            finish(conversation)


def handle_interrupt(request, conversation):
    p = request.get("params", {})
    append_line(home / "interrupts.jsonl", json.dumps(p) + "\n")
    if scenario.get("complete_before_interrupt") and conversation.active_turn:
        finish(conversation)
    if not conversation.active_turn or p["turnId"] != conversation.turn_id:
        emit({"id": request["id"], "error": {"code": -32600, "message": "no matching active turn"}})
        return
    if scenario.get("reject_interrupt"):
        emit({"id": request["id"], "error": {"code": -1, "message": "already ending"}})
        time.sleep(0.1)
    else:
        reply(request, {})
    if scenario.get("wrong_completion"):
        emit(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turn": {"id": "wrong-turn", "status": "interrupted"},
                },
            }
        )
        return False
    if conversation.command_child and not scenario.get("retain_shell_on_interrupt"):
        conversation.command_child.terminate()
        conversation.command_child.wait()
        conversation.command_child = None
    finish(conversation, "interrupted")
    if scenario.get("late_immediate"):
        late_terminal(conversation)
    if conversation.steering_request is not None:
        if scenario.get("steer_ack_gate"):
            threading.Thread(
                target=gated_ack,
                args=(
                    conversation.steering_request,
                    conversation.turn_id,
                    scenario["steer_ack_gate"],
                ),
                daemon=True,
            ).start()
        else:
            reply(conversation.steering_request, {"turnId": conversation.turn_id})
        conversation.steering_request = None


def handle_clean(request, conversation):
    if scenario.get("unexpected_turn"):
        emit(
            {
                "method": "turn/started",
                "params": {
                    "threadId": conversation.thread_id,
                    "turn": {"id": "unexpected", "status": "inProgress"},
                },
            }
        )
    if scenario.get("cleanup_refused"):
        emit({"id": request["id"], "error": {"code": -1, "message": "cleanup refused"}})
    else:
        if conversation.command_child:
            conversation.command_child.terminate()
            conversation.command_child.wait()
            conversation.command_child = None
        reply(request, {})


def handle_unsubscribe(request, conversation):
    if not scenario.get("retain_loaded_after_unsubscribe"):
        loaded_threads.discard(conversation.thread_id)
    reply(request, {"status": "unsubscribed"})


def handle_loaded_threads(request, conversation):
    reply(request, {"data": sorted(loaded_threads), "nextCursor": None})


def handle_terminals(request, conversation):
    conversation.list_calls += 1
    if scenario.get("exiting_terminal") and conversation.list_calls == 1:
        reply(request, {"data": [{"id": "exiting"}]})
    elif scenario.get("omit_cursor"):
        reply(request, {"data": []})
    else:
        reply(request, {"data": [], "nextCursor": None})
    if scenario.get("list_marker"):
        publish_text(pathlib.Path(scenario["list_marker"]), str(conversation.list_calls))
    if scenario.get("late_after_list") == conversation.list_calls:
        late_terminal(conversation)
        for _ in range(scenario.get("late_burst", 0)):
            emit(
                {
                    "method": "item/completed",
                    "params": {
                        "threadId": conversation.thread_id,
                        "turnId": conversation.turn_id,
                        "item": {"id": "late-command", "type": "commandExecution"},
                    },
                }
            )
    if scenario.get("late_continuous"):
        emit(
            {
                "method": "item/completed",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "item": {"id": "late-telemetry", "type": "commandExecution"},
                },
            }
        )


def handle_unsupported(request, conversation):
    emit({"id": request["id"], "error": {"code": -32601, "message": "unsupported"}})


HANDLERS = {
    "initialize": handle_initialize,
    "account/read": handle_account,
    "account/rateLimits/read": handle_rate_limits,
    "model/list": handle_models,
    "thread/start": handle_thread,
    "thread/resume": handle_thread,
    "thread/fork": handle_thread,
    "turn/start": handle_turn_start,
    "turn/steer": handle_steer,
    "turn/interrupt": handle_interrupt,
    "thread/backgroundTerminals/clean": handle_clean,
    "thread/unsubscribe": handle_unsubscribe,
    "thread/loaded/list": handle_loaded_threads,
    "thread/backgroundTerminals/list": handle_terminals,
}


def record_input(conversation, p):
    body = p["input"][0]["text"].split(":\n", 1)[1]
    sender = p["input"][0]["text"].split(" from ", 1)[1].split(":\n", 1)[0]
    append_line(home / "inputs.jsonl", json.dumps(
                {
                    "thread": conversation.thread_id,
                    "body": body,
                    "input": p["input"][0]["text"],
                    "native_input": p["input"],
                    "at": time.time(),
                }
            )
            + "\n", max_record_bytes=INPUT_RECORD_LIMIT)
    if body.startswith("Hive branch identity:"):
        assert conversation.meta.get("forked_from"), "branch child must never use thread/start"
        assert f'You are agent {conversation.meta["agent"]},' in body
        body = body.split("\n\nNew task:\n", 1)[1]
    conversation.meta.setdefault("history", []).append(body)
    save(conversation)
    return body.split("\n\nHive attachment references", 1)[0], sender


def quota_error(conversation, body):
    if (scenario.get("quota_once") or body == scenario.get("quota_body")) and not (
        home / "quota-seen"
    ).exists():
        publish_text(home / "quota-seen", "1")
        reset = scenario.get("quota_until", int(time.time()) + scenario.get("quota_seconds", 3600))
        if scenario.get("quota_error", "usageLimitExceeded") == "usageLimitExceeded":
            emit(
                {
                    "method": "account/rateLimits/updated",
                    "params": {
                        "rateLimits": {
                            "primary": {"usedPercent": 100, "resetsAt": reset},
                            "rateLimitReachedType": "rate_limit_reached",
                        }
                    },
                }
            )
        emit(
            {
                "method": "error",
                "params": {
                    "threadId": conversation.thread_id,
                    "turnId": conversation.turn_id,
                    "willRetry": False,
                    "error": {"codexErrorInfo": scenario.get("quota_error", "usageLimitExceeded")},
                },
            }
        )
        if scenario.get("quota_disconnect"):
            sys.exit(0)
        finish(conversation, "failed")
        return True
    return False


conversation = Conversation()
HANDLERS.update({"configRequirements/read": handle_requirements, "config/read": handle_config,
                 "account/login/start": handle_api_login})
for line in sys.stdin:
    request = json.loads(line)
    conversation = select_conversation(request, conversation)
    method = request.get("method")
    if method and scenario.get("trace"):
        append_line(scenario["trace"], method + "\n")
    if method is None and "result" in request:
        handler = handle_tool_result
    elif method and "id" in request:
        handler = HANDLERS.get(method, handle_unsupported)
    else:
        handler = HANDLERS.get(method)
    if handler is not None and handler(request, conversation) is False:
        break


if scenario.get("linger_eof") and conversation.thread_id is not None:
    if scenario.get("eof_stop_file"):
        wait_for_gate(scenario["eof_stop_file"], timeout=15)
    else:
        time.sleep(3)
if scenario.get("malformed_eof") and conversation.thread_id is not None:
    print("malformed protocol", flush=True)
if scenario.get("unexpected_eof") and conversation.thread_id is not None:
    emit(
        {
            "method": "turn/started",
            "params": {
                "threadId": conversation.thread_id,
                "turn": {"id": "unexpected", "status": "inProgress"},
            },
        }
    )
if scenario.get("require_sigterm"):
    time.sleep(30)
