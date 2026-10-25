use super::*;
use crate::runtime::{provider::shard::corpus_support, test_support::RuntimeFixture};

#[tokio::test]
async fn cancelled_attach_retires_empty_owner_but_keeps_its_charged_slot() {
    for peer in [false, true] {
        let fixture = RuntimeFixture::new();
        let pool = fixture.pool();
        let corpus_support::ShardSetup { shard, mut queues } = corpus_support::shard(&fixture);
        shard.occupancy().slots.insert("agent".into());
        shard.occupancy().users.insert("agent".into());
        if peer {
            shard.occupancy().users.insert("peer".into());
        }
        let mut future = Box::pin(pool.attach(shard.clone(), "agent"));
        let command = tokio::select! {
            _ = &mut future => panic!("Attach unexpectedly finished"),
            command = queues.normal.recv() => command,
        };
        assert!(matches!(command, Some(Cmd::Attach(..))));
        drop(future);
        assert_eq!(shard.occupancy().users.is_empty(), !peer);
        assert!(shard.occupancy().slots.contains("agent"));
        assert_eq!(shard.draining(), !peer);
        assert_eq!(shard.abandoned_attach(), !peer);
        assert!(!shard.stop());
        drop(command);
        drop(pool);
        drop(shard);
        drop(queues);
        fixture.finish();
    }
}
