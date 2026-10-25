use super::*;

fn text(item: &str, text: &str) -> Event {
    Event {
        turn_id: Some("turn".into()),
        item_id: Some(item.into()),
        text: Some(text.into()),
        ..Event::new(Kind::Text)
    }
}

const NOW: Duration = Duration::ZERO;

#[tokio::test]
async fn unwatched_agents_build_nothing_and_attach_opens_a_bounded_channel() {
    let hub = LiveHub::default();
    let mut built = false;
    hub.publish("a", || {
        built = true;
        Some(text("i", "x"))
    });
    assert!(!built && !hub.watched());
    let cursor = hub.open("a").unwrap().0;
    assert!(hub.watched());
    assert_eq!(
        (cursor.epoch, cursor.seq, cursor.durable),
        (hub.epoch(), 0, 0)
    );
    // Other agents stay unwatched even while one channel exists.
    hub.publish("b", || {
        built = true;
        Some(text("i", "x"))
    });
    assert!(!built);
    hub.publish("a", || Some(text("i", "hello")));
    let page = hub.next("a", cursor, NOW, MAX_PAGE_ITEMS).await.unwrap();
    assert!(!page.resync && !page.durable_changed);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].text.as_deref(), Some("hello"));
    assert_eq!(page.cursor.unwrap().seq, 1);
}

#[tokio::test]
async fn ring_drops_oldest_by_count_and_bytes_and_behind_cursors_resync() {
    let hub = LiveHub::default();
    let start = hub.open("a").unwrap().0;
    for index in 0..(RING_ITEMS + 10) {
        hub.publish("a", || Some(text(&index.to_string(), "x")));
    }
    let page = hub.next("a", start, NOW, MAX_PAGE_ITEMS).await.unwrap();
    assert!(page.resync && page.items.is_empty());
    let current = page.cursor.unwrap();
    assert_eq!(current.seq, u64::try_from(RING_ITEMS + 10).unwrap());
    let behind = Cursor { seq: 9, ..current };
    assert!(hub.next("a", behind, NOW, 1).await.unwrap().resync);
    let oldest = Cursor { seq: 10, ..current };
    let page = hub.next("a", oldest, NOW, 3).await.unwrap();
    assert!(!page.resync);
    assert_eq!(
        page.items.iter().map(|i| i.seq).collect::<Vec<_>>(),
        [10, 11, 12]
    );
    // Byte budget: large items evict earlier ones before the count limit.
    let big = "y".repeat(ITEM_TEXT_BYTES * 2);
    for index in 0..100 {
        hub.publish("a", || Some(text(&format!("big{index}"), &big)));
    }
    let channels = hub.channels.lock_or_panic();
    let channel = &channels["a"];
    assert!(channel.bytes <= RING_BYTES);
    assert!(channel.ring.len() < 100);
    assert!(channel.ring.iter().all(|item| item.truncated));
    assert!(
        channel
            .ring
            .iter()
            .all(|item| item.text.as_ref().unwrap().len() == ITEM_TEXT_BYTES)
    );
}

#[tokio::test]
async fn deltas_of_one_item_merge_and_others_stay_separate() {
    let hub = LiveHub::default();
    let cursor = hub.open("a").unwrap().0;
    hub.publish("a", || Some(text("m1", "Hel")));
    hub.publish("a", || Some(text("m1", "lo")));
    hub.publish("a", || {
        Some(Event {
            item_id: Some("c1".into()),
            tool: Some("commandExecution".into()),
            ..Event::new(Kind::ToolStarted)
        })
    });
    hub.publish("a", || Some(text("m2", "x")));
    let page = hub.next("a", cursor, NOW, MAX_PAGE_ITEMS).await.unwrap();
    let kinds: Vec<_> = page
        .items
        .iter()
        .map(|i| (i.kind, i.text.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            (Kind::Text, Some("Hello".into())),
            (Kind::ToolStarted, None),
            (Kind::Text, Some("x".into())),
        ]
    );
    assert_eq!(page.items[0].seq, 1);
    assert_eq!(page.cursor.unwrap().seq, 4);
}

#[tokio::test]
async fn epoch_generation_and_missing_channels_resync() {
    let hub = LiveHub::default();
    let cursor = hub.open("a").unwrap().0;
    let wrong_epoch = Cursor {
        epoch: cursor.epoch ^ 1,
        ..cursor
    };
    let page = hub.next("a", wrong_epoch, NOW, 1).await.unwrap();
    assert!(page.resync);
    assert_eq!(page.cursor, Some(cursor));
    // A re-created channel never accepts the old channel's cursor.
    hub.discard("a", cursor.generation);
    assert!(!hub.watched());
    let missing = hub.next("a", cursor, NOW, 1).await.unwrap();
    assert!(missing.resync && missing.cursor.is_none());
    let reopened = hub.open("a").unwrap().0;
    assert_ne!(reopened.generation, cursor.generation);
    hub.publish("a", || Some(text("i", "x")));
    assert!(hub.next("a", cursor, NOW, 1).await.unwrap().resync);
    // A stale discard leaves the new channel alone.
    hub.discard("a", cursor.generation);
    assert!(hub.watched());
}

#[tokio::test]
async fn expired_leases_are_swept_and_restore_the_zero_viewer_path() {
    let hub = LiveHub::default();
    hub.open("a").unwrap();
    hub.open("b").unwrap();
    hub.channels
        .lock_or_panic()
        .get_mut("a")
        .unwrap()
        .lease_until = Instant::now();
    hub.durable_changed(["b"], false);
    assert_eq!(hub.viewers.load(Ordering::Acquire), 1);
    hub.channels
        .lock_or_panic()
        .get_mut("b")
        .unwrap()
        .lease_until = Instant::now();
    hub.open("c").unwrap();
    hub.discard("c", 3);
    assert!(!hub.watched());
    assert!(hub.channels.lock_or_panic().is_empty());
}

#[tokio::test]
async fn durable_changes_wake_current_viewers_without_items() {
    let hub = LiveHub::default();
    let a = hub.open("a").unwrap().0;
    let b = hub.open("b").unwrap().0;
    hub.durable_changed(["a", "unwatched"], false);
    let page = hub.next("a", a, NOW, 1).await.unwrap();
    assert!(page.durable_changed && page.items.is_empty() && !page.resync);
    assert_eq!(page.cursor.unwrap().durable, 1);
    // A viewer that is current waits, then wakes for an overflow bump.
    let waiting = hub.next("b", b, Duration::from_secs(5), 1);
    let bump = async {
        tokio::task::yield_now().await;
        hub.durable_changed([], true);
    };
    let (page, ()) = tokio::join!(waiting, bump);
    assert!(page.unwrap().durable_changed);
    // A timed-out wait returns the same cursor and no change.
    let current = hub.next("a", page_cursor(&hub, "a"), Duration::from_millis(10), 1);
    let page = current.await.unwrap();
    assert!(page.items.is_empty() && !page.durable_changed && !page.resync);
}

fn page_cursor(hub: &LiveHub, agent: &str) -> Cursor {
    hub.channels.lock_or_panic()[agent].cursor(hub.epoch())
}

#[tokio::test]
async fn capacity_limits_channels_and_long_polls() {
    let hub = LiveHub::default();
    for index in 0..CHANNEL_LIMIT {
        hub.open(&index.to_string()).unwrap();
    }
    assert_eq!(hub.open("over").unwrap_err().code(), "watch_capacity");
    // Renewing an existing channel is always allowed.
    hub.open("0").unwrap();
    let permits: Vec<_> = (0..LONG_POLL_LIMIT).map(|_| hub.admit().unwrap()).collect();
    assert_eq!(hub.admit().unwrap_err().code(), "watch_capacity");
    // A request that cannot wait needs no permit, even with every one taken.
    assert!(hub.admit_if(false).unwrap().is_none());
    assert_eq!(hub.admit_if(true).unwrap_err().code(), "watch_capacity");
    drop(permits);
    assert!(hub.admit_if(true).unwrap().is_some());
    drop(hub.admit().unwrap());
}

#[tokio::test]
async fn tree_revisions_wait_debounce_and_stop() {
    let hub = LiveHub::default();
    let first = hub.tree_after(None, MAX_WAIT).await.unwrap();
    assert_eq!(first.epoch, hub.epoch());
    let other_epoch = TreeVersion {
        epoch: first.epoch ^ 1,
        ..first
    };
    assert_eq!(
        hub.tree_after(Some(other_epoch), MAX_WAIT).await.unwrap(),
        first
    );
    let waiting = hub.tree_after(Some(first), Duration::from_secs(5));
    let change = async {
        tokio::task::yield_now().await;
        hub.tree_changed();
    };
    let (next, ()) = tokio::join!(waiting, change);
    assert_eq!(next.unwrap().revision, first.revision + 1);
    // Stopping wakes both kinds of long-poll at once.
    let cursor = hub.open("a").unwrap().0;
    let live = hub.next("a", cursor, MAX_WAIT, 1);
    let tree = hub.tree_after(Some(hub.tree_version()), MAX_WAIT);
    let stop = async {
        tokio::task::yield_now().await;
        hub.stop();
    };
    let (live, tree, ()) = tokio::join!(live, tree, stop);
    assert_eq!(live.unwrap_err().code(), "daemon_stopping");
    assert_eq!(tree.unwrap_err().code(), "daemon_stopping");
    assert_eq!(
        hub.next("a", cursor, NOW, 1).await.unwrap_err().code(),
        "daemon_stopping"
    );
}
