use super::corpus_support::{RequestSetup, ShardSetup, request, route, with_driver};
use crate::runtime::provider::protocol::{IncomingFrame, outgoing};
use crate::runtime::{provider::RpcHarness, test_support::RuntimeFixture};
use serde_json::json;
#[test]
fn codex_fork_queue_failure_is_unsent_but_wakeup_failure_is_ambiguous() {
    use super::{corpus_support::shard, init::ShardChannels, *};
    for closed in [true, false] {
        let fixture = RuntimeFixture::new();
        let ShardSetup { shard, queues } = shard(&fixture);
        let ShardChannels {
            normal,
            priority,
            essential,
            wake,
        } = queues;
        drop(wake);
        let held = (!closed).then_some((normal, priority, essential));
        let (tx, _rx) = oneshot::channel();
        let error = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(shard.command(
                Cmd::Call(
                    "agent".into(),
                    outgoing::thread_fork(None, "source", "cut"),
                    tx,
                ),
                false,
            ))
            .unwrap_err();
        assert_eq!(error.input_was_not_submitted(), closed);
        if let Some((mut normal, _, _)) = held {
            assert!(matches!(
                normal.try_recv().unwrap(),
                Cmd::Call(_, outgoing::OutgoingRequest::ThreadFork(_), _)
            ));
        }
        drop(shard);
        fixture.finish();
    }
}

#[test]
fn claude_closed_request_queue_is_unsent_but_failed_wakeup_is_ambiguous() {
    use super::{corpus_support::shard, init::ShardChannels, *};
    use crate::runtime::provider::{Backend, claude::SessionConfig};
    for closed in [true, false] {
        let fixture = RuntimeFixture::new();
        let ShardSetup { mut shard, queues } = shard(&fixture);
        Arc::get_mut(&mut shard).unwrap().backend = Backend::Claude(Arc::new(SessionConfig {
            instructions: String::new(),
            ..SessionConfig::for_test("reserved", fixture.temp.path().into())
        }));
        let ShardChannels {
            normal,
            priority,
            essential,
            wake,
        } = queues;
        drop(wake);
        let held = (!closed).then_some((normal, priority, essential));
        let (tx, _rx) = oneshot::channel();
        let error = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(shard.command(
                Cmd::Call("agent".into(), outgoing::turn_input("thread", ""), tx),
                false,
            ))
            .unwrap_err();
        assert_eq!(error.input_was_not_submitted(), closed);
        drop(held);
        drop(shard);
        fixture.finish();
    }
}

#[test]
fn held_calls_and_internal_interrupts_both_consume_request_capacity() {
    use super::{ClientMethod, Pending, in_flight};
    use std::{collections::HashMap, time::Instant};
    let mut pending = HashMap::new();
    for (id, external, deadline) in [
        (1, true, false),
        (2, true, true),
        (3, false, true),
        (4, false, false),
    ] {
        let (tx, _) = tokio::sync::oneshot::channel();
        pending.insert(
            id,
            Pending {
                agent: "agent".into(),
                method: ClientMethod::TurnInterrupt,
                reply: external.then_some(tx),
                deadline: deadline.then(Instant::now),
                thread: Some("thread".into()),
            },
        );
    }
    assert_eq!(in_flight(&pending), 3);
}

#[test]
fn quota_telemetry_and_stale_pre_ack_errors_do_not_invent_cutoff_inputs() {
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::active(launcher.clone());
    with_driver(&fixture, &mut rpc, |driver, _| {
        route(driver,IncomingFrame::parse(json!({"method":"hive/claudeRateLimits","params":{"threadId":"thread","turnId":"turn","info":{"status":"rejected","resetsAt":9_999_999_999_u64,"rateLimitType":"five_hour"}}}))).unwrap();
        fixture
            .hub
            .blocking_with(|core| {
                let mut agent = core.store().agent("agent")?;
                assert!(agent.auth_cutoff.is_none());
                assert_eq!(
                    core.store()
                        .availability("main", agent.auth_generation)?
                        .state,
                    crate::runtime::auth_fallback::AvailabilityState::Limited
                );
                agent.auth_group = Some("plans".into());
                agent.turn_id = None;
                agent.turn_selection = Some(agent.desired_selection());
                agent.reserved_turn = Some(crate::runtime::domain::ReservedTurn {
                    message_id: "message".into(),
                    selection: agent.desired_selection(),
                });
                core.store().save(&agent)
            })
            .unwrap();
        route(driver,IncomingFrame::parse(json!({"method":"error","params":{"threadId":"thread","turnId":"old-completed-turn","willRetry":false,"error":{"codexErrorInfo":"usageLimitExceeded"}}}))).unwrap();
        fixture
            .hub
            .blocking_with(|core| {
                let mut agent = core.store().agent("agent")?;
                assert_eq!(
                    agent
                        .auth_cutoff
                        .as_ref()
                        .map(crate::runtime::domain::AuthCutoff::turn),
                    Some("old-completed-turn")
                );
                assert!(
                    agent
                        .auth_cutoff
                        .as_ref()
                        .and_then(crate::runtime::domain::AuthCutoff::message)
                        .is_none()
                );
                core.auth_uncertain_cutoff(&mut agent)?;
                assert_eq!(
                    core.store().messages("agent", None, 100)?["messages"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
                Ok(())
            })
            .unwrap();
    });
    fixture.finish();
}

#[test]
fn typed_turn_rpc_rejection_records_hold_without_claiming_safe_replay_or_cutoff() {
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::new(launcher.clone());
    rpc.open();
    with_driver(&fixture, &mut rpc, |driver, _| {
        let RequestSetup { id, mut reply } =
            request(driver, outgoing::turn_input("thread", ""), false);
        route(driver,IncomingFrame::parse(json!({"id":id,"error":{"code":-32000,"message":"quota","data":{"codexErrorInfo":"usageLimitExceeded"}}}))).unwrap();
        let error = reply.try_recv().unwrap().err().unwrap();
        assert_eq!(error.code(), "provider_request_failed");
        assert!(!error.input_was_not_submitted());
    });
    fixture
        .hub
        .blocking_with(|core| {
            let a = core.store().agent("agent")?;
            assert!(a.auth_cutoff.is_none());
            assert!(
                a.auth_cutoff
                    .as_ref()
                    .and_then(crate::runtime::domain::AuthCutoff::message)
                    .is_none()
            );
            assert_eq!(
                core.store().availability("main", a.auth_generation)?.state,
                crate::runtime::auth_fallback::AvailabilityState::Limited
            );
            Ok(())
        })
        .unwrap();
    fixture.finish();
}
