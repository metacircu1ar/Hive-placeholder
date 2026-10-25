#!/usr/bin/env python3
"""Deterministic native-wire canary peer; never invokes a provider or network."""

import json
import os
import pathlib
import re
import sys
import time

case = sys.argv[1]
args = sys.argv[2:]
session = args[args.index("--session-id") + 1]
instructions = pathlib.Path(args[args.index("--append-system-prompt-file") + 1]).read_text()
turn = None
prompt = None
model = "claude-sonnet-test"


def emit(value):
    print(json.dumps(value), flush=True)


def control(request_id, response):
    emit(
        {
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request_id, "response": response},
        }
    )


def hook(event, callback, request_id):
    emit(
        {
            "type": "control_request",
            "request_id": request_id,
            "request": {
                "subtype": "hook_callback",
                "callback_id": callback,
                "input": {
                    "session_id": session,
                    "hook_event_name": event,
                    "tool_name": "Read",
                    "tool_use_id": "canary-read",
                    "tool_input": {"file_path": str(pathlib.Path.cwd() / "fixed.txt")},
                },
            },
        }
    )


for line in sys.stdin:
    frame = json.loads(line)
    if frame.get("type") == "control_request":
        request = frame["request"]
        subtype = request["subtype"]
        if subtype == "initialize":
            control(
                frame["request_id"],
                {
                    "account": {"subscriptionType": "Team", "apiProvider": "firstParty"},
                    "models": [
                        {
                            "value": "sonnet",
                            "resolvedModel": model,
                            "displayName": "Sonnet",
                            "description": "deterministic mock",
                            "supportedEffortLevels": ["low"],
                        }
                    ],
                },
            )
        elif subtype == "get_settings":
            if case == "cancel-preflight":
                pathlib.Path("settings-waiting").touch()
                deadline = time.monotonic() + 5
                while not pathlib.Path("release-settings").exists():
                    assert time.monotonic() < deadline, "test never released settings"
                    time.sleep(0.005)
            control(frame["request_id"], {"applied": {"model": model, "effort": "low"}})
        elif subtype == "interrupt":
            control(frame["request_id"], {})
            emit(
                {
                    "type": "result",
                    "session_id": session,
                    "user_message_uuid": turn,
                    "subtype": "success",
                    "is_error": False,
                    "terminal_reason": "aborted_streaming",
                }
            )
    elif frame.get("type") == "user":
        assert case not in ("cancelled", "cancel-preflight"), "cancelled canary submitted inference"
        pathlib.Path("inference-submitted").touch()
        turn = frame["uuid"]
        prompt = frame["message"]["content"]
        emit(
            {
                "type": "system",
                "subtype": "init",
                "session_id": session,
                "model": model,
                "permissionMode": "default",
            }
        )
        emit({"type": "user", "uuid": turn, "session_id": session, "isReplay": True})
        emit(
            {
                "type": "assistant",
                "session_id": session,
                "message": {
                    "model": model,
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "canary-read",
                            "name": "Read",
                            "input": {"file_path": str(pathlib.Path.cwd() / "fixed.txt")},
                        }
                    ],
                },
            }
        )
        hook("PreToolUse", "hive-guard", "pre")
    elif frame.get("type") == "control_response":
        request_id = frame["response"]["request_id"]
        if request_id == "pre":
            emit(frame)
            emit(
                {
                    "type": "user",
                    "session_id": session,
                    "message": {
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "canary-read",
                                "content": "fixed harmless content",
                            }
                        ]
                    },
                }
            )
            hook("PostToolUse", "hive-finished", "post")
        elif request_id == "post":
            guidance = frame["response"]["response"]["hookSpecificOutput"]["additionalContext"]
            nonce = re.search(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", guidance).group(0)
            assert nonce not in prompt
            assert nonce not in instructions
            assert nonce not in pathlib.Path("fixed.txt").read_text()
            assert all(nonce not in value for value in os.environ.values())
            if case in ("echo", "missing", "synthetic"):
                emit(frame)
            text = "No marker here." if case == "missing" else "Final answer " + nonce
            emit(
                {
                    "type": "assistant",
                    "session_id": session,
                    "message": {
                        "model": "<synthetic>" if case == "synthetic" else model,
                        "content": [{"type": "text", "text": text}],
                    },
                }
            )
            emit(
                {
                    "type": "result",
                    "session_id": session,
                    "user_message_uuid": turn,
                    "subtype": "success",
                    "is_error": False,
                    "terminal_reason": "completed",
                }
            )
