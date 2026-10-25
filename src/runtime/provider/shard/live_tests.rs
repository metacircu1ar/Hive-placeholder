//! Live deltas publish in provider read order, even around deferred frames.
use super::corpus_support::with_driver;
use crate::runtime::live::{Cursor, Event, Kind, Page};
use crate::runtime::provider::live_tap::Delta;
use crate::runtime::provider::protocol::IncomingFrame;
use crate::runtime::{provider::RpcHarness, test_support::RuntimeFixture};
use serde_json::json;
use std::time::Duration;

fn command(method: &str) -> serde_json::Value {
    json!({"method": method, "params": {"threadId": "thread", "turnId": "turn",
        "item": {"type": "commandExecution", "id": "c", "command": "cargo test",
            "cwd": "/w", "commandActions": [], "status": "completed", "exitCode": 0}}})
}

fn delta(kind: Kind) -> Delta {
    Delta {
        thread: "thread".into(),
        event: Event {
            turn_id: Some("turn".into()),
            item_id: Some("i".into()),
            text: Some("x".into()),
            ..Event::new(kind)
        },
    }
}

fn page(driver: &super::ShardLoop<'_>, after: Cursor) -> Page {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(
            driver
                .pool
                .wake
                .live
                .next("agent", after, Duration::ZERO, 64),
        )
        .unwrap()
}

#[test]
fn deltas_publish_before_later_deferred_frames_and_lost_deltas_resync_viewers() {
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::active(launcher);
    with_driver(&fixture, &mut rpc, |driver, _output| {
        let start = driver.pool.wake.live.open("agent").unwrap().0;
        // Read order: started (1), output (2), completed (3), text (4). The two
        // frames were deferred by a synchronous call, the deltas side-queued.
        for (seq, method) in [(1, "item/started"), (3, "item/completed")] {
            let frame = IncomingFrame::parse(command(method)).with_seq(seq);
            let bytes = frame.encoded_len();
            driver.rpc.pending.push_back(frame, bytes);
        }
        driver.rpc.deltas.push(2, delta(Kind::ToolOutput));
        driver.rpc.deltas.push(4, delta(Kind::Text));
        driver.drain_frames().unwrap();
        // Deltas alone keep the shard from sleeping until they are published.
        driver.rpc.deltas.push(5, delta(Kind::Text));
        assert!(driver.rpc.buffered());
        driver.drain_frames().unwrap();
        assert!(!driver.rpc.buffered());
        let first = page(driver, start);
        let kinds: Vec<_> = first.items.iter().map(|item| item.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::ToolStarted,
                Kind::ToolOutput,
                Kind::ToolCompleted,
                Kind::Text
            ]
        );
        // A full side queue drops its oldest delta; that thread's viewers resync.
        let current = first.cursor.unwrap();
        for seq in 0..=1024 {
            driver.rpc.deltas.push(5 + seq, delta(Kind::Text));
        }
        driver.drain_frames().unwrap();
        let resynced = page(driver, current);
        assert!(resynced.resync && resynced.items.is_empty());
        let after = resynced.cursor.unwrap();
        assert!(after.seq > current.seq);
        assert!(!page(driver, after).resync);
    });
}
