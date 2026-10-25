//! Exact raw-macro exceptions are inventoried for privacy attack regressions.
use super::*;
use std::{error::Error, fmt};

struct MustNotFormat;
impl fmt::Debug for MustNotFormat {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("must not Debug raw value")
    }
}
impl fmt::Display for MustNotFormat {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("must not Display raw value")
    }
}
impl Error for MustNotFormat {}

#[test]
fn dependencies_and_missed_lints_cannot_serialize_payloads_or_formatters() {
    let (backend, guard, memory) = scoped(Config {
        level: LevelFilter::TRACE,
        ..Config::default()
    });
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend.clone()), || {
        let span = tracing::info_span!("test.raw.span", password = ?MustNotFormat);
        let _entered = span.enter();
        tracing::warn!(target: "hyper", secret = ?MustNotFormat, token = "sk-secret-token", agent_id = "sk-secret-id", count = 123_456, "sk-secret-library-message");
        tracing::error!(target: "reqwest", error = %MustNotFormat, "sk-secret-http-body");
        // Raw native error references use classification, not formatting.
        tracing::error!(name: "test.native.error", error = &MustNotFormat as &(dyn Error + 'static));
        tracing::warn!(name: "test.missed.lint", code = ?MustNotFormat);
    });
    assert_eq!(backend.counts().invalid, 1);
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    let records = memory.records();
    assert_eq!(records[0]["event"], "library.event");
    assert_eq!(records[0]["target"], "hyper");
    assert_eq!(records[0]["fields_redacted"], 5);
    assert_eq!(records[2]["error"]["items"][0]["kind"], "other");
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains("sk-secret"));
    assert!(!all.contains("123456"));
    assert!(!all.contains("password"));
}

#[test]
fn library_verbosity_is_independent_and_overrides_are_target_specific() {
    for enabled in [false, true] {
        let mut config = Config {
            level: LevelFilter::TRACE,
            ..Config::default()
        };
        if enabled {
            config
                .library_levels
                .insert("hyper".into(), LevelFilter::DEBUG);
        }
        let (backend, guard, memory) = scoped(config);
        tracing::subscriber::with_default(tracing_subscriber::registry().with(backend), || {
            tracing::debug!(name: "app.debug", count = 1);
            tracing::trace!(target: "hyper", count = panic_count());
            tracing::debug!(target: "reqwest", count = panic_count());
            tracing::debug!(target: "hyper", count = if enabled { 1 } else { panic_count() });
            tracing::warn!(target: "reqwest", "sk-secret-connection");
            for target in ["hyper", "reqwest", "hyper_extra"] {
                tracing_log::format_trace(
                    &log::Record::builder()
                        .level(log::Level::Debug)
                        .target(target)
                        .args(format_args!("sk-secret-log"))
                        .build(),
                )
                .unwrap();
            }
        });
        assert_eq!(
            guard.finish(Duration::from_secs(2)).state,
            crate::Shutdown::Drained
        );
        let records = memory.records();
        assert_eq!(records.len(), if enabled { 4 } else { 2 });
        assert_eq!(
            records
                .iter()
                .filter(|r| r["level"] == "debug" && r["event"] == "library.event")
                .count(),
            if enabled { 2 } else { 0 }
        );
        assert!(
            !serde_json::to_string(&records)
                .unwrap()
                .contains("sk-secret")
        );
    }
}

#[test]
fn bridge_dynamic_metadata_cannot_leak_or_claim_first_party_privileges() {
    let (backend, guard, memory) = scoped(Config {
        level: LevelFilter::TRACE,
        ..Config::default()
    });
    tracing::subscriber::with_default(tracing_subscriber::registry().with(backend.clone()), || {
        for target in ["sk-secret-target", "hive.fatal", "hive::runtime"] {
            tracing_log::format_trace(
                &log::Record::builder()
                    .level(log::Level::Warn)
                    .target(target)
                    .module_path(Some("sk-secret-module"))
                    .file(Some("sk-secret-file"))
                    .args(format_args!("{MustNotFormat}"))
                    .build(),
            )
            .unwrap();
            tracing_log::format_trace(
                &log::Record::builder()
                    .level(log::Level::Debug)
                    .target(target)
                    .args(format_args!("sk-secret-debug"))
                    .build(),
            )
            .unwrap();
        }
    });
    assert_eq!(backend.counts().fatal_unconfirmed, 0);
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        crate::Shutdown::Drained
    );
    let records = memory.records();
    assert_eq!(records.len(), 3);
    for record in &records {
        assert_eq!(record["event"], "library.event");
        assert_eq!(record["target"], "log");
        assert!(record.get("location").is_none());
    }
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("sk-secret")
    );
}
