use super::*;
use crate::{Admission, Fields, Invalid, Level, Shutdown, record::encode};
use std::{error::Error, fmt};

struct SecretError;
impl fmt::Display for SecretError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("secret Display must never be called")
    }
}
impl fmt::Debug for SecretError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("secret Debug must never be called")
    }
}
impl Error for SecretError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

#[test]
fn disabled_is_lazy_and_errors_never_render_secret_text() {
    let (logger, guard, memory) = start_memory(Config::default());
    assert_eq!(
        logger.record(Level::Debug, "ignored", |_| panic!("disabled closure")),
        Admission::Disabled
    );
    assert_eq!(
        logger.record(Level::Info, "test.secret", |f| f.cause(&SecretError)),
        Admission::Queued
    );
    assert_eq!(
        logger.record(Level::Error, "test.io", |f| f
            .cause(&io::Error::other("sk-secret-private-page"))),
        Admission::Queued
    );
    let json_error = serde_json::from_str::<u64>("\"sk-secret-provider\"").unwrap_err();
    assert_eq!(
        logger.record(Level::Warn, "test.json", |f| f.cause(&json_error)),
        Admission::Queued
    );
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        Shutdown::Drained
    );
    let records = memory.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["cause"]["items"].as_array().unwrap().len(), 8);
    assert_eq!(records[0]["cause"]["truncated"], true);
    assert_eq!(records[1]["cause"]["items"][0]["kind"], "io");
    assert_eq!(records[2]["cause"]["items"][0]["kind"], "json_data");
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("sk-secret")
    );
    assert!(records[0]["t"].as_u64().unwrap() > 0);
}
#[test]
fn field_identity_event_and_serialized_limits_are_separate() {
    let mut fields = Fields::default();
    assert_eq!(
        fields.identity("agent_id", "raw\nprivate body"),
        Err(Invalid::Identity)
    );
    assert_eq!(fields.count("event", 1), Err(Invalid::Name));
    fields.identity("agent_id", "a-1").unwrap();
    assert_eq!(fields.identity("agent_id", "a-2"), Err(Invalid::Duplicate));
    assert_eq!(
        encode(Level::Info, "Bad Event", fields, 8192),
        Err(Invalid::Name)
    );
    let (logger, guard, memory) = start_memory(Config {
        record_bytes: 1024,
        ..Config::default()
    });
    assert_eq!(logger.record(Level::Info, "valid", |f| {
        for key in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"] {
            f.code(key, "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")?;
        }
        Ok(())
    }), Admission::Oversized);
    assert_eq!(
        logger.record(Level::Info, "Invalid event", |_| Ok(())),
        Admission::Invalid
    );
    let finished = guard.finish(Duration::from_secs(2));
    assert_eq!((finished.counts.oversized, finished.counts.invalid), (1, 1));
    assert_eq!(memory.records()[0]["event"], "log.dropped");
}
#[test]
fn limits_fail_before_creating_any_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    for config in [
        Config {
            queue_slots: usize::MAX,
            ..Config::default()
        },
        Config {
            record_bytes: 8192,
            file_bytes: 8192,
            ..Config::default()
        },
        Config {
            loss_interval: Duration::ZERO,
            ..Config::default()
        },
    ] {
        assert!(crate::guard::start(&path, config).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
