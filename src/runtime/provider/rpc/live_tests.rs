//! Displayed deltas are consumed before lifecycle observation and routing.
use super::Rpc;
use crate::runtime::{hub::Wake, live::Kind};
use std::{process::Command, sync::Arc};

/// A stub provider that prints these frames, then stays open.
fn provider(frames: &[serde_json::Value]) -> (Arc<crate::launch::LauncherCache>, Rpc) {
    let launcher = crate::launch::test_cache().unwrap();
    let lines: Vec<String> = frames.iter().map(ToString::to_string).collect();
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "printf '%s\\n' \"$@\"; exec sleep 30", "stub"])
        .args(lines);
    let rpc = Rpc::start(&launcher, command, vec![]).unwrap();
    (launcher, rpc)
}

fn frames() -> Vec<serde_json::Value> {
    let delta = |method: &str, thread: &str| {
        serde_json::json!({"method": method, "params":
            {"threadId": thread, "turnId": "turn", "itemId": "item", "delta": "x"}})
    };
    vec![
        delta("item/agentMessage/delta", "unknown-thread"),
        delta("item/reasoning/textDelta", "unknown-thread"),
        delta("item/commandExecution/outputDelta", "unknown-thread"),
        serde_json::json!({"method": "configWarning", "params": {}}),
    ]
}

fn next_frame(rpc: &mut Rpc) -> crate::runtime::provider::protocol::IncomingFrame {
    for _ in 0..200 {
        if let Some(frame) = rpc.next().unwrap() {
            return frame;
        }
        rpc.wait(None, std::time::Duration::from_millis(50))
            .unwrap();
    }
    panic!("stub provider produced no frame");
}

#[test]
fn watched_deltas_queue_without_observation_and_unknown_threads_stay_healthy() {
    let wake = Arc::new(Wake::default());
    wake.live.open("agent").unwrap();
    let (_launcher, mut rpc) = provider(&frames());
    rpc.stream_live(wake);
    let frame = next_frame(&mut rpc);
    assert_eq!(frame.raw()["method"], "configWarning");
    assert!(!rpc.uncertain());
    let deltas = rpc.deltas.before(None).deltas;
    let kinds: Vec<_> = deltas.iter().map(|delta| delta.event.kind).collect();
    // Raw reasoning is dropped even when a provider sends it after opting out.
    assert_eq!(kinds, [Some(Kind::Text), Some(Kind::ToolOutput)]);
    assert!(deltas.iter().all(|delta| delta.thread == "unknown-thread"));
}

#[test]
fn unwatched_deltas_are_dropped_and_disabled_streaming_keeps_old_routing() {
    let (_launcher, mut rpc) = provider(&frames());
    rpc.stream_live(Arc::new(Wake::default()));
    assert_eq!(next_frame(&mut rpc).raw()["method"], "configWarning");
    assert!(rpc.deltas.before(None).deltas.is_empty() && !rpc.uncertain());
    // Without live streaming, deltas stay ordinary (opted-out) notifications.
    let (_launcher, mut rpc) = provider(&frames());
    assert_eq!(
        next_frame(&mut rpc).raw()["method"],
        "item/agentMessage/delta"
    );
}
