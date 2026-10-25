use super::*;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::prelude::*;
mod canaries;
mod install;

fn scoped(config: Config) -> (crate::Backend, Guard, Memory) {
    let (logger, guard, memory) = start_memory(config);
    (crate::subscriber::from_logger(logger), guard, memory)
}

#[test]
fn macros_capture_fields_and_filter_before_evaluation() {
    let (backend, guard, memory) = scoped(Config::default());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
        tracing::debug!(name: "test.disabled", count = panic_count());
        tracing::info!(name: "agent.spawned", agent_id = "a-1", count = 3_u64, success = true, decision = "accepted");
        let error = io::Error::other("sk-secret-error-body");
        tracing::error!(name: "test.error", error = &error as &(dyn std::error::Error + 'static));
    });
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    let records = memory.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["agent_id"], "a-1");
    assert_eq!(records[0]["count"], 3);
    assert_eq!(records[0]["success"], true);
    assert_eq!(records[1]["error"]["items"][0]["kind"], "io");
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("sk-secret")
    );
}
fn panic_count() -> u64 {
    panic!("disabled field was evaluated")
}

#[test]
fn native_duration_and_numeric_types_are_preserved_with_explicit_range_rejection() {
    let (backend, guard, memory) = scoped(Config::default());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend.clone()), || {
        let elapsed = Duration::from_millis(1250);
        tracing::info!(name: "test.duration", elapsed_ms = elapsed.as_millis(), duration_ms = elapsed.as_secs_f64(), count = -42_i128);
        tracing::info!(name: "test.boundaries", count = u128::from(u64::MAX), errno = i128::from(i64::MIN));
        tracing::info!(name: "test.overflow", count = u128::MAX);
        tracing::info!(name: "test.underflow", count = i128::MIN);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            tracing::info!(name: "test.nonfinite", elapsed_ms = value);
        }
    });
    assert_eq!(backend.counts().numeric_invalid, 5);
    assert_eq!(backend.counts().invalid, 5);
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    let records = memory.records();
    let duration = records
        .iter()
        .find(|r| r["event"] == "test.duration")
        .unwrap();
    assert_eq!(duration["elapsed_ms"], 1250);
    assert_eq!(duration["duration_ms"], 1.25);
    assert_eq!(duration["count"], -42);
    let boundary = records
        .iter()
        .find(|r| r["event"] == "test.boundaries")
        .unwrap();
    assert_eq!(boundary["count"], u64::MAX);
    assert_eq!(boundary["errno"], i64::MIN);
    assert!(!records.iter().any(|r| r["event"] == "test.overflow"
        || r["event"] == "test.underflow"
        || r["event"] == "test.nonfinite"));
}

#[test]
fn standard_span_context_is_bounded_and_respects_parent_selection() {
    let (backend, guard, memory) = scoped(Config {
        level: LevelFilter::TRACE,
        ..Config::default()
    });
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
        let mut spans: Vec<tracing::Span> = Vec::new();
        for _ in 0..128 {
            let parent = spans.last().and_then(tracing::Span::id);
            spans.push(tracing::info_span!(parent: parent, "test.span"));
        }
        let last = spans.last().unwrap();
        let _entered = last.enter();
        tracing::info!(name: "test.deep", count = 1);
        tracing::info!(name: "test.none", parent: None, count = 2);
        tracing::info!(name: "test.explicit", parent: &spans[1], count = 3);
    });
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    let records = memory.records();
    assert_eq!(records[0]["spans"].as_array().unwrap().len(), 8);
    assert_eq!(records[0]["spans_truncated"], true);
    assert!(records[1].get("spans").is_none());
    assert_eq!(records[2]["spans"].as_array().unwrap().len(), 2);
    assert_eq!(records[2]["spans_truncated"], false);
}

#[test]
fn tracing_fatal_bypasses_disabled_level_and_a_full_normal_queue() {
    let memory = Memory::default();
    let (sink, entered, release) = Blocked::new(memory.clone());
    let stderr = Memory::default();
    let config = Config {
        queue_slots: 1,
        ..Config::default()
    };
    let (logger, guard) =
        start_with(config, Box::new(sink), Box::new(stderr.clone()), &Threads).unwrap();
    let backend = crate::subscriber::from_logger(logger);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend.clone()), || {
        tracing::info!(name: "test.blocked", count = 1);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        tracing::info!(name: "test.queued", count = 1);
        tracing::info!(name: "test.full", count = 1);
        let now = Instant::now();
        tracing::error!(name: "test.fatal", target: "hive.fatal", decision = "shutdown");
        assert!(now.elapsed() < Duration::from_secs(1));
    });
    assert_eq!(backend.counts().full, 1);
    assert_eq!(stderr.records()[0]["event"], "test.fatal");
    release.send(()).unwrap();
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    let (backend, guard, memory) = scoped(Config {
        level: LevelFilter::OFF,
        ..Config::default()
    });
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
        tracing::error!(name: "test.normal.disabled", count = panic_count());
        tracing::error!(name: "test.fatal.off", target: "hive.fatal", count = 1);
    });
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    assert_eq!(memory.records().len(), 1);
    assert_eq!(memory.records()[0]["event"], "test.fatal.off");
}

#[test]
fn tracing_reports_an_unconfirmed_fatal_when_both_sinks_block() {
    let (sink, entered, release) = Blocked::new(Memory::default());
    let (stderr, stderr_entered, stderr_release) = Blocked::new(Memory::default());
    let (logger, guard) = start_with(
        Config::default(),
        Box::new(sink),
        Box::new(stderr),
        &Threads,
    )
    .unwrap();
    let backend = crate::subscriber::from_logger(logger);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend.clone()), || {
        tracing::info!(name: "test.block", count = 1);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        tracing::error!(name: "test.fatal.blocked", target: "hive.fatal", count = 1);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(backend.counts().fatal_unconfirmed, 1);
    });
    stderr_entered.recv_timeout(Duration::from_secs(1)).unwrap();
    release.send(()).unwrap();
    stderr_release.send(()).unwrap();
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
}
