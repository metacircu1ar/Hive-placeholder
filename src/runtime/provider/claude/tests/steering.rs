use super::*;

fn fixture(mode: ReceiptMode) -> Fixture {
    Fixture::with_steering(Some(SteeringPermit::Verified(mode)))
}
fn steer(f: &mut Fixture, id: u64, turn: &str, text: &str) {
    assert!(
        f.codec
            .send(outgoing::request(id, outgoing::steer(&f.id, turn, text)))
            .unwrap()
            .is_empty()
    );
}
fn tool(f: &mut Fixture) {
    f.codec.receive(json!({"type":"assistant","session_id":f.id,"message":{
        "model":"claude-sonnet-5","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"/tmp/fixed"}}]}})).unwrap();
    assert_eq!(f.codec.pop().unwrap()["method"], "item/started");
    assert_eq!(
        f.hook("Read", json!({"file_path":"/tmp/fixed"}))["permissionDecision"],
        "allow"
    );
}
fn post(f: &mut Fixture, event: &str, id: &str) -> Value {
    f.codec
        .receive(json!({"type":"control_request","request_id":id,"request":{
        "subtype":"hook_callback","callback_id":"hive-finished","input":{
        "hook_event_name":event,"session_id":f.id,"tool_use_id":"tool-1","tool_name":"Read"}}}))
        .unwrap()
        .remove(0)
}
fn text(f: &mut Fixture) {
    f.codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "model":"claude-sonnet-5","content":[{"type":"text","text":"final"}]}}))
        .unwrap();
}
fn end(f: &mut Fixture, turn: &str, reason: &str) {
    f.codec
        .receive(
            json!({"type":"result","session_id":f.id,"user_message_uuid":turn,
        "subtype":"success","is_error":false,"terminal_reason":reason}),
        )
        .unwrap();
}
#[test]
fn guidance_is_fifo_once_and_tool_ownership_survives_tool_result() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "first");
    steer(&mut f, 11, &turn, "second");
    assert!(f.codec.pop().is_none());
    tool(&mut f);
    f.codec.receive(json!({"type":"user","session_id":f.id,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"fixed"}]}})).unwrap();
    f.codec.pop();
    let reply = post(&mut f, "PostToolUse", "post");
    assert_eq!(
        reply["response"]["response"]["hookSpecificOutput"]["additionalContext"],
        "first\n\nsecond"
    );
    assert!(f.codec.pop().is_none());
    f.codec.written(&reply);
    text(&mut f);
    assert_eq!(f.codec.pop().unwrap()["method"], "item/completed");
    assert!(
        f.codec.pop().is_none(),
        "echo mode cannot downgrade to continuation"
    );
    f.codec.receive(reply.clone()).unwrap();
    for id in [10, 11] {
        let ack = f.codec.pop().unwrap();
        assert_eq!(ack["id"], id);
        assert_eq!(ack["result"]["turnId"], turn);
        assert_eq!(ack["result"]["deliveryEvidence"]["hookReply"], reply);
    }
    f.codec.receive(reply).unwrap();
    assert!(f.codec.pop().is_none());
    assert!(
        post(&mut f, "PostToolUse", "duplicate")["response"]["response"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    end(&mut f, &turn, "completed");
}
#[test]
fn no_boundary_and_interrupted_held_guidance_are_stale() {
    for interrupt in [false, true] {
        let mut f = fixture(ReceiptMode::NativeEcho);
        let turn = f.turn();
        steer(&mut f, 10, &turn, "held");
        if interrupt {
            f.codec
                .send(outgoing::request(12, outgoing::interrupt(&f.id, &turn)))
                .unwrap();
        }
        end(
            &mut f,
            &turn,
            if interrupt {
                "aborted_streaming"
            } else {
                "completed"
            },
        );
        let ack = f.codec.pop().unwrap();
        assert_eq!(ack["id"], 10);
        assert_eq!(ack["error"]["message"], "no active turn to steer");
        assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
    }
}
#[test]
fn failure_unowned_and_previous_turn_hooks_never_deliver() {
    for failure in [false, true] {
        let mut f = fixture(ReceiptMode::NativeEcho);
        let turn = f.turn();
        steer(&mut f, 10, &turn, "held");
        if failure {
            tool(&mut f);
        }
        let reply = post(
            &mut f,
            if failure {
                "PostToolUseFailure"
            } else {
                "PostToolUse"
            },
            "post",
        );
        assert!(
            reply["response"]["response"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        end(&mut f, &turn, "completed");
        assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32600);
    }
}
#[test]
fn written_continuation_needs_both_events_and_cancellation_is_uncertain() {
    for cancel in [false, true] {
        let mut f = fixture(ReceiptMode::WrittenContinuation);
        let turn = f.turn();
        steer(&mut f, 10, &turn, "held");
        tool(&mut f);
        let reply = post(&mut f, "PostToolUse", "post");
        text(&mut f);
        assert_eq!(f.codec.pop().unwrap()["method"], "item/completed");
        assert!(f.codec.pop().is_none());
        f.codec.written(&reply);
        if cancel {
            f.codec
                .receive(json!({"type":"control_cancel_request","request_id":"post"}))
                .unwrap();
            assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32000);
            f.codec.receive(reply).unwrap();
            assert!(f.codec.pop().is_none());
        } else {
            f.codec.receive(reply).unwrap();
            assert!(
                f.codec.pop().is_none(),
                "configured receipt mode remains exact"
            );
            text(&mut f);
            assert_eq!(
                f.codec.pop().unwrap()["result"]["deliveryEvidence"]["kind"],
                "written_continuation"
            );
            f.codec.pop();
        }
        end(&mut f, &turn, "completed");
    }
}
#[test]
fn partial_stream_burst_never_queues_frames_or_confirms_continuation() {
    let mut f = fixture(ReceiptMode::WrittenContinuation);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    tool(&mut f);
    let reply = post(&mut f, "PostToolUse", "post");
    f.codec.written(&reply);
    // Deliberately do not drain the consumer while ten thousand chunks arrive.
    // The codec must discard every partial frame before any shared queue.
    for _ in 0..10_000 {
        assert!(
            f.codec
                .receive(json!({"type":"stream_event","session_id":f.id,
            "user_message_uuid":turn,"event":{"type":"content_block_delta",
            "delta":{"type":"text_delta","text":"partial"}}}))
                .unwrap()
                .is_empty()
        );
    }
    assert!(!f.codec.buffered());
    text(&mut f);
    let receipt = f.codec.pop().unwrap();
    assert_eq!(receipt["id"], 10);
    assert_eq!(
        receipt["result"]["deliveryEvidence"]["kind"],
        "written_continuation"
    );
    assert_eq!(f.codec.pop().unwrap()["method"], "item/completed");
    end(&mut f, &turn, "completed");
    assert_eq!(
        f.codec.pop().unwrap()["params"]["turn"]["status"],
        "completed"
    );
    assert!(!f.codec.buffered());
}
#[test]
fn emitted_without_receipt_is_uncertain_and_never_replayed() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    tool(&mut f);
    let reply = post(&mut f, "PostToolUse", "post");
    end(&mut f, &turn, "completed");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32000);
    f.codec.pop();
    f.codec.receive(reply).unwrap();
    assert!(f.codec.pop().is_none());
}
#[test]
fn unsupported_default_and_wrong_turn_are_rejected_without_holding() {
    let mut f = Fixture::new();
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32601);
    let mut f = fixture(ReceiptMode::NativeEcho);
    f.turn();
    steer(&mut f, 10, "other-turn", "held");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32600);
}

#[test]
fn native_terminal_fences_guidance_before_shell_cleanup_finishes() {
    let mut f = fixture(ReceiptMode::WrittenContinuation);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    tool(&mut f);
    f.codec.receive(json!({"type":"user","session_id":f.id,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"fixed"}]}})).unwrap();
    f.codec.pop();
    let cleanup = f.codec.guard.expect().unwrap();
    end(&mut f, &turn, "completed");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32600);
    assert!(
        f.codec.pop().is_none(),
        "terminal event still waits for shell cleanup"
    );
    let reply = post(&mut f, "PostToolUse", "late-post");
    assert!(
        reply["response"]["response"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    f.codec.receive(reply).unwrap();
    assert!(f.codec.pop().is_none());
    f.codec.guard.revoke(&cleanup);
    f.codec.settle();
    assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
}
#[test]
fn verified_tool_content_or_successful_completion_can_confirm_full_write() {
    for followup in ["tool", "thinking", "completion"] {
        let mut f = fixture(ReceiptMode::WrittenContinuation);
        let turn = f.turn();
        steer(&mut f, 10, &turn, "held");
        tool(&mut f);
        let reply = post(&mut f, "PostToolUse", "post");
        f.codec.written(&reply);
        if followup == "completion" {
            end(&mut f, &turn, "completed");
        } else {
            let block = if followup == "tool" {
                json!({"type":"tool_use","id":"next-tool","name":"Read","input":{}})
            } else {
                json!({"type":"thinking","thinking":"work"})
            };
            f.codec.receive(json!({"type":"assistant","session_id":f.id,"message":{"model":"claude-sonnet-5","content":[block]}})).unwrap();
        }
        assert_eq!(
            f.codec.pop().unwrap()["result"]["deliveryEvidence"]["kind"],
            "written_continuation"
        );
    }
}
#[test]
fn synthetic_text_and_interruption_do_not_confirm_full_write() {
    let mut f = fixture(ReceiptMode::WrittenContinuation);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    tool(&mut f);
    let reply = post(&mut f, "PostToolUse", "post");
    f.codec.written(&reply);
    f.codec.receive(json!({"type":"assistant","session_id":f.id,"message":{"model":"<synthetic>","content":[{"type":"text","text":"provider error"}]}})).unwrap();
    assert_eq!(f.codec.pop().unwrap()["method"], "item/completed");
    assert!(f.codec.pop().is_none());
    end(&mut f, &turn, "aborted_streaming");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32000);
}
#[test]
fn full_batch_rejects_only_the_extra_request() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let turn = f.turn();
    for id in 10..26 {
        steer(&mut f, id, &turn, "held");
    }
    steer(&mut f, 26, &turn, "extra");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32602);
    tool(&mut f);
    let reply = post(&mut f, "PostToolUse", "post");
    f.codec.receive(reply).unwrap();
    for id in 10..26 {
        assert_eq!(f.codec.pop().unwrap()["id"], id);
    }
    end(&mut f, &turn, "completed");
    assert_eq!(
        f.codec.pop().unwrap()["params"]["turn"]["status"],
        "completed"
    );
}

#[test]
fn interrupt_immediately_stales_held_guidance_without_terminal_confirmation() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    f.codec
        .send(outgoing::request(11, outgoing::interrupt(&f.id, &turn)))
        .unwrap();
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32600);
    assert!(f.codec.pop().is_none());
}
#[test]
fn old_turn_post_hook_cannot_consume_new_turn_guidance() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let old = f.turn();
    tool(&mut f);
    f.codec.receive(json!({"type":"user","session_id":f.id,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1"}]}})).unwrap();
    f.codec.pop();
    end(&mut f, &old, "completed");
    f.codec.pop();
    let current = f.turn();
    steer(&mut f, 10, &current, "new turn");
    assert!(
        post(&mut f, "PostToolUse", "late-old-hook")["response"]["response"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(f.codec.pop().is_none());
    end(&mut f, &current, "completed");
    assert_eq!(f.codec.pop().unwrap()["error"]["code"], -32600);
}

#[test]
fn reused_tool_identity_is_rejected_before_late_hooks_can_rebind_it() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let old = f.turn();
    tool(&mut f);
    post(&mut f, "PostToolUse", "old-post");
    end(&mut f, &old, "completed");
    f.codec.pop();
    let current = f.turn();
    steer(&mut f, 10, &current, "new guidance");
    let reused = json!({"type":"assistant","session_id":f.id,"message":{"model":"claude-sonnet-5",
        "content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{}}]}});
    assert!(f.codec.receive(reused).is_err());
    assert!(
        post(&mut f, "PostToolUse", "delayed-old-post")["response"]["response"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    f.codec.receive(json!({"type":"user","session_id":f.id,"message":{"content":[{"type":"tool_result","tool_use_id":"tool-1"}]}})).unwrap();
    assert_eq!(f.codec.pop().unwrap()["method"], "item/completed");
    assert!(
        f.codec.pop().is_none(),
        "old result cannot acknowledge new guidance"
    );
}

#[test]
fn pre_hook_before_assistant_observation_is_upgraded_only_within_its_turn() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let turn = f.turn();
    steer(&mut f, 10, &turn, "held");
    assert_eq!(
        f.hook("Read", json!({"file_path":"/tmp/fixed"}))["permissionDecision"],
        "allow"
    );
    f.codec
        .receive(
            json!({"type":"assistant","session_id":f.id,"message":{"model":"claude-sonnet-5",
        "content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{}}]}}),
        )
        .unwrap();
    f.codec.pop();
    let reply = post(&mut f, "PostToolUse", "post");
    assert_eq!(
        reply["response"]["response"]["hookSpecificOutput"]["additionalContext"],
        "held"
    );
    f.codec.receive(reply).unwrap();
    assert_eq!(f.codec.pop().unwrap()["id"], 10);
}
#[test]
fn escaped_guidance_is_bounded_on_the_wire_without_killing_the_session() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    let turn = f.turn();
    let body = "\u{0001}".repeat(65_536);
    let mut rejected = Vec::new();
    for id in 10..26 {
        steer(&mut f, id, &turn, &body);
        if let Some(reply) = f.codec.pop() {
            assert_eq!(reply["error"]["code"], -32602);
            rejected.push(id);
        }
    }
    assert!(!rejected.is_empty());
    tool(&mut f);
    let reply = post(&mut f, "PostToolUse", "post");
    assert!(reply.to_string().len() < 4 * 1024 * 1024);
    f.codec.receive(reply).unwrap();
    for id in (10..26).filter(|id| !rejected.contains(id)) {
        assert_eq!(f.codec.pop().unwrap()["id"], id);
    }
    end(&mut f, &turn, "completed");
    assert_eq!(
        f.codec.pop().unwrap()["params"]["turn"]["status"],
        "completed"
    );
}
#[test]
fn oversized_native_tool_ids_are_rejected_before_retention() {
    let mut f = fixture(ReceiptMode::NativeEcho);
    f.turn();
    assert!(
        f.codec
            .receive(
                json!({"type":"assistant","session_id":f.id,"message":{"model":"claude-sonnet-5",
        "content":[{"type":"tool_use","id":"x".repeat(1025),"name":"Read","input":{}}]}})
            )
            .is_err()
    );
}
