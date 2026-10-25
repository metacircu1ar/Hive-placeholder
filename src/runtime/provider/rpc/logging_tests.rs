use super::*;

#[test]
fn native_stderr_and_lenient_fields_produce_bounded_private_diagnostics() {
    const CHILD: &str = "HIVE_PROVIDER_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut command = crate::test_support::rerun_isolated(
            &format!(
                "{module}::native_stderr_and_lenient_fields_produce_bounded_private_diagnostics"
            ),
            false,
        );
        crate::test_support::run_isolated(command.env(CHILD, "1"));
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("diagnostics.jsonl");
    let (logger, guard) = hive_log::start(
        &path,
        hive_log::Config {
            level: tracing::level_filters::LevelFilter::DEBUG,
            ..Default::default()
        },
    )
    .unwrap();
    logger.install_global().unwrap();
    let bytes = stderr_canary();
    malformed_fields();
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("private-"));
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let stderr: Vec<_> = rows
        .iter()
        .filter(|row| row["event"] == "provider.stderr_tail")
        .collect();
    assert_eq!(stderr.len(), 1);
    assert_eq!(stderr[0]["bytes"], bytes);
    assert_eq!(stderr[0]["decision"], "discard_private_text");
    let drift: Vec<_> = rows
        .iter()
        .filter(|row| row["event"] == "provider.protocol_drift")
        .collect();
    assert_eq!(drift.len(), 10);
    assert_eq!(drift.last().unwrap()["count"], 512);
}

fn stderr_canary() -> usize {
    let body = "private-native-stderr-canary";
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "printf '%s' \"$1\" >&2; cat >/dev/null",
        "fixture",
        body,
    ]);
    let launcher = crate::launch::test_cache().unwrap();
    let mut rpc = Rpc::start(&launcher, command, vec![]).unwrap();
    crate::test_support::wait_until(Duration::from_secs(5), Duration::from_millis(1), || {
        rpc.next().unwrap();
        (rpc.pipes.stderr_bytes > 0).then_some(())
    });
    assert!(rpc.close().confirmed);
    rpc.close(); // Idempotent close must not duplicate the byte summary.
    body.len()
}

fn malformed_fields() {
    #[derive(serde::Deserialize)]
    struct Item {
        #[serde(deserialize_with = "super::super::protocol::lenient::optional")]
        count: Option<u64>,
        retained: u64,
    }
    for _ in 0..1000 {
        let item: Item = serde_json::from_value(serde_json::json!({
            "count": "private-field-canary", "retained": 7,
        }))
        .unwrap();
        assert!(item.count.is_none());
        assert_eq!(item.retained, 7);
    }
}
