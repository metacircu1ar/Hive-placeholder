use super::*;
use crate::runtime::{
    provider::{RpcHarness, shard::corpus_support},
    test_support::{RuntimeFixture, failure_log},
};

#[test]
fn bootstrap_retries_model_list_after_authentication_and_keeps_one_copy_per_page() {
    use crate::runtime::provider::protocol::IncomingFrame;
    use serde_json::json;
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    for raw in [
        json!({"id":1,"result":{}}),
        json!({"id":2,"result":{"account":{"type":"chatgpt","email":"fixture@example.test"},"requiresOpenaiAuth":true}}),
        json!({"id":3,"error":{"code":-1,"message":"fixture rejection"}}),
        json!({"id":4,"result":{"data":[{"model":"first"}],"nextCursor":"second"}}),
        json!({"id":5,"result":{"data":[{"model":"second"}],"nextCursor":null}}),
    ] {
        let frame = IncomingFrame::parse(raw);
        let bytes = frame.encoded_len();
        harness.rpc.pending.push_back(frame, bytes);
    }
    let home = tempfile::tempdir().unwrap();
    let models = bootstrap(
        &mut harness.rpc,
        &crate::auth::Credential::default(),
        home.path(),
    )
    .unwrap();
    assert_eq!(models.len(), 2);
    let requests = harness.outgoing(6); // includes initialized notification
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "initialize")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "account/read")
            .count(),
        1
    );
    let pages: Vec<_> = requests
        .iter()
        .filter(|r| r["method"] == "model/list")
        .collect();
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[0]["params"], pages[1]["params"]);
    assert_eq!(pages[2]["params"]["cursor"], "second");
}

fn setup(fixture: &RuntimeFixture) -> corpus_support::ShardSetup {
    let mut setup = corpus_support::shard(fixture);
    Arc::get_mut(&mut setup.shard).unwrap().id = Uuid::new_v4().to_string();
    setup
}
fn binding(fixture: &RuntimeFixture) -> state::Binding {
    state::State::new(fixture.temp.path().join("state"))
        .unwrap()
        .lookup("main", &crate::runtime::test_support::agent().profile)
        .unwrap()
        .unwrap()
}
fn fail_writes(fixture: &RuntimeFixture, table: &'static str) {
    fixture.hub.blocking_with(move |core| {
        core.store().test_connection().execute_batch(&format!(
            "CREATE TRIGGER fail_record BEFORE {} ON {table} BEGIN SELECT RAISE(ABORT, 'secret provider SQL'); END;",
            if table == "agents" { "UPDATE" } else { "INSERT" }
        )).map_err(|cause| Error::database_error("fixture").with_source(cause))
    }).unwrap();
}
#[test]
fn startup_failures_retain_catalog_error_and_check_failed_quarantine() {
    let Some(log) = failure_log(
        module_path!(),
        "startup_failures_retain_catalog_error_and_check_failed_quarantine",
        || {
            for reservation in [false, true] {
                let fixture = RuntimeFixture::new();
                let pool = fixture.pool();
                let corpus_support::ShardSetup { shard, queues } = if reservation {
                    corpus_support::shard(&fixture)
                } else {
                    setup(&fixture)
                };
                run_shard(
                    &pool,
                    &shard,
                    ThreadInputs {
                        credential: crate::auth::Credential::default(),
                        command: Command::new(fixture.temp.path().join("absent-provider")),
                        launcher: fixture.launcher.clone(),
                        launch_owner: None,
                        lease: fixture.lease.clone(),
                        daemon_lock: fixture.daemon_lock.clone(),
                        channels: queues,
                    },
                );
                assert_eq!(
                    shard.catalog.get().unwrap().as_ref().unwrap_err().code(),
                    if reservation {
                        "state_inconsistent"
                    } else {
                        "spawn_failed"
                    }
                );
                assert!(shard.capacity_released());
                assert!(shard.exited());
                drop(pool);
                drop(shard);
                fixture.finish();
            }
            let fixture = RuntimeFixture::new();
            let pool = fixture.pool();
            let corpus_support::ShardSetup {
                shard,
                queues: _queues,
            } = corpus_support::shard(&fixture);
            fail_writes(&fixture, "agents");
            quarantine_after_failure(&pool, &shard, &Error::state_io("secret launch error"));
            assert!(shard.draining());
            assert!(!shard.capacity_released());
            assert_eq!(fixture.hub.wake.failure().unwrap().code(), "database_error");
            drop(pool);
            drop(shard);
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("provider.start_failed"));
    assert!(log.contains("provider.quarantine_record_failed"));
    assert!(!log.contains("secret provider SQL"));
    assert!(!log.contains("secret launch error"));
}

#[test]
fn shutdown_failures_still_reap_and_settle_release_replies() {
    let Some(log) = failure_log(
        module_path!(),
        "shutdown_failures_still_reap_and_settle_release_replies",
        || {
            let fixture = RuntimeFixture::new();
            let pool = fixture.pool();
            let corpus_support::ShardSetup { shard, mut queues } = setup(&fixture);
            let mut rpc = RpcHarness::new(fixture.launcher.clone());
            let started = Started {
                pid: rpc.rpc.pid(),
                before: 0,
                after: 0,
            };
            let mut evidence = fixture.lease.begin_provider(&shard.id).unwrap();
            evidence.started(started.pid).unwrap();
            std::fs::write(
                binding(&fixture)
                    .directory
                    .join(format!("executions/{}.json", shard.id)),
                "secret damaged receipt",
            )
            .unwrap();
            fail_writes(&fixture, "provider_events");
            let (reply, released) = tokio::sync::oneshot::channel();
            shard
                .essential
                .blocking_send(crate::runtime::provider::shard::Cmd::Release {
                    agent: "agent".into(),
                    last: true,
                    clean: true,
                    reply,
                })
                .unwrap();
            let mut driver = ShardLoop::new(&pool, &shard, &mut rpc.rpc, &started, &mut queues);
            driver.shutdown(
                &mut evidence,
                Err(Error::provider_request_failed("secret driver failure")),
            );
            let report = released.blocking_recv().unwrap();
            assert!(report.reaped);
            assert!(!report.confirmed);
            assert!(ProcessIdentity::read(started.pid).unwrap().is_none());
            assert!(shard.capacity_released());
            assert!(shard.exited());
            assert_eq!(fixture.hub.wake.failure().unwrap().code(), "database_error");
            drop(driver);
            drop(rpc);
            drop(pool);
            drop(shard);
            drop(evidence);
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("provider.execution_failed"));
    assert!(log.contains("provider.receipt_finish_failed"));
    assert!(log.contains("provider.shutdown_record_failed"));
    assert!(!log.contains("secret damaged receipt"));
    assert!(!log.contains("secret driver failure"));
    assert!(!log.contains("secret provider SQL"));
}

#[test]
fn unstarted_receipt_and_branch_failures_do_not_create_safe_retry_proof() {
    let Some(log) = failure_log(
        module_path!(),
        "unstarted_receipt_and_branch_failures_do_not_create_safe_retry_proof",
        || {
            for corrupt_receipt in [false, true] {
                let fixture = RuntimeFixture::new();
                let pool = fixture.pool();
                let corpus_support::ShardSetup {
                    shard,
                    queues: _queues,
                } = setup(&fixture);
                let mut evidence = fixture.lease.begin_provider(&shard.id).unwrap();
                if corrupt_receipt {
                    std::fs::write(
                        binding(&fixture)
                            .directory
                            .join(format!("executions/{}.json", shard.id)),
                        "secret broken receipt",
                    )
                    .unwrap();
                }
                let owner = LaunchOwner {
                    agent: "absent-agent".into(),
                    execution: "execution".into(),
                };
                ShardThread {
                    pool: &pool,
                    shard: &shard,
                    launcher: &fixture.launcher,
                    lease: &fixture.lease,
                    owner: Some(&owner),
                }
                .unstarted(&mut evidence, Error::spawn_failed("secret launch failure"));
                let error = shard.catalog.get().unwrap().as_ref().unwrap_err();
                assert_eq!(error.code(), "spawn_failed");
                assert!(!error.input_was_not_submitted());
                assert!(shard.capacity_released());
                assert!(shard.exited());
                assert_eq!(
                    shard.report.lock_or_panic().as_ref().unwrap().confirmed,
                    !corrupt_receipt
                );
                if corrupt_receipt {
                    assert!(shard.draining());
                } else {
                    assert!(fixture.hub.wake.failure().is_some());
                }
                drop(evidence);
                drop(pool);
                drop(shard);
                fixture.finish();
            }
        },
    ) else {
        return;
    };
    assert!(log.contains("provider.unstarted_receipt_failed"));
    assert!(log.contains("provider.branch_settlement_failed"));
    assert!(!log.contains("secret broken receipt"));
    assert!(!log.contains("secret launch failure"));
}

#[test]
fn failed_started_record_publishes_cause_and_fences_even_if_shutdown_record_succeeds() {
    let Some(log) = failure_log(
        module_path!(),
        "failed_started_record_publishes_cause_and_fences_even_if_shutdown_record_succeeds",
        || {
            let fixture = RuntimeFixture::new();
            fixture.hub.blocking_with(|core| {
            core.store().test_connection().execute_batch(
                "CREATE TRIGGER fail_started BEFORE INSERT ON provider_events WHEN NEW.kind = 'started' BEGIN SELECT RAISE(ABORT, 'secret started SQL'); END;"
            ).map_err(|cause| Error::database_error("fixture").with_source(cause))
        }).unwrap();
            let pool = fixture.pool();
            let corpus_support::ShardSetup { shard, queues } = setup(&fixture);
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "cat >/dev/null"]);
            run_shard(
                &pool,
                &shard,
                ThreadInputs {
                    credential: crate::auth::Credential::default(),
                    command,
                    launcher: fixture.launcher.clone(),
                    launch_owner: None,
                    lease: fixture.lease.clone(),
                    daemon_lock: fixture.daemon_lock.clone(),
                    channels: queues,
                },
            );
            assert_eq!(
                shard.catalog.get().unwrap().as_ref().unwrap_err().code(),
                "database_error"
            );
            assert_eq!(fixture.hub.wake.failure().unwrap().code(), "database_error");
            fixture
                .hub
                .blocking_with(|core| {
                    assert!(core.shutdown().load(Ordering::Acquire));
                    let kinds: Vec<String> = core
                        .store()
                        .test_connection()
                        .prepare("SELECT kind FROM provider_events ORDER BY seq")
                        .unwrap()
                        .query_map([], |row| row.get(0))
                        .unwrap()
                        .map(std::result::Result::unwrap)
                        .collect();
                    assert_eq!(kinds, ["shutdown"]);
                    Ok(())
                })
                .unwrap();
            assert!(shard.capacity_released());
            assert!(shard.exited());
            let pid = shard.process.lock_or_panic().as_ref().unwrap().pid;
            assert!(ProcessIdentity::read(pid).unwrap().is_none());
            drop(pool);
            drop(shard);
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("record_started"));
    assert!(!log.contains("provider.shutdown_record_failed"));
    assert!(!log.contains("secret started SQL"));
}
