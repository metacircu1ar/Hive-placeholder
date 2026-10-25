use super::corpus_support::{RequestSetup, RouteOutput, RouteSetup, request, route, with_driver};
use crate::runtime::provider::protocol::{IncomingFrame, outgoing};
use crate::runtime::{provider::RpcHarness, test_support::RuntimeFixture};
use serde_json::json;
use std::sync::atomic::Ordering;
#[test]
fn cancelled_reattach_does_not_turn_late_output_into_peer_failure() {
    use super::Route;
    use std::sync::{Arc, atomic::AtomicBool};
    let launcher = crate::launch::test_cache().unwrap();
    for installed in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::active(launcher.clone());
        let completed = rpc
            .ingest(
                &json!({"method":"turn/completed","params":{"threadId":"thread",
                "turn":{"id":"turn","status":"completed"}}}),
            )
            .unwrap();
        with_driver(&fixture, &mut rpc, |driver, _output| {
            route(driver, IncomingFrame::parse(completed)).unwrap();
            driver.routes.remove("agent");
            let cancelled = Arc::new(AtomicBool::new(!installed));
            let rx = attach(driver, cancelled.clone());
            // Model PendingAttach's token-before-receiver drop ordering.
            cancelled.store(true, Ordering::Release);
            drop(rx);
            let RouteSetup {
                route: peer,
                output:
                    RouteOutput {
                        events: _events,
                        failure: peer_failure,
                    },
            } = Route::for_test(1, Arc::default());
            driver.routes.insert("peer".into(), peer);
            for completed in [
                json!({"method":"item/completed","params":{"threadId":"thread",
                    "turnId":"turn","item":{"type":"agentMessage","id":"late","text":"done"}}}),
                json!({"method":"item/completed","params":{"threadId":"thread",
                    "turnId":"turn","item":{"type":"agentMessage","id":"later","text":"done"}}}),
            ] {
                route(driver, IncomingFrame::parse(completed)).unwrap();
                assert!(!driver.shard.draining());
                assert!(peer_failure.borrow().is_none());
                assert!(!driver.rpc.uncertain());
            }
            // A fresh attachment owns its own token and receives its active turn's output.
            let mut rx = attach(driver, Arc::default());
            let RequestSetup {
                id,
                reply: mut started,
            } = request(driver, outgoing::turn_input("thread", ""), false);
            route(
                driver,
                IncomingFrame::parse(json!({"id":id,"result":{"turn":{"id":"next-turn"}}})),
            )
            .unwrap();
            assert_eq!(
                started.try_recv().unwrap().unwrap().turn_id(),
                Some("next-turn")
            );
            let output = json!({"method":"item/completed","params":{"threadId":"thread",
                "turnId":"next-turn","item":{"type":"agentMessage","id":"later","text":"done"}}});
            route(driver, IncomingFrame::parse(output.clone())).unwrap();
            assert_eq!(rx.try_recv().unwrap().value.into_raw(), output);
        });
        fixture.finish();
    }
}

#[test]
fn empty_reuse_revalidates_before_release_dispatch_without_consuming_peer_acks() {
    use super::{Cmd, Route};
    use crate::runtime::provider::protocol::IncomingFrame;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::oneshot;
    let launcher = crate::launch::test_cache().unwrap();
    for overdue in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::new(launcher.clone());
        with_driver(&fixture, &mut rpc, |driver, _| {
            // The retired owner's route is still present. First reuse must
            // consume its obligation even if account verification is fresh.
            driver.shard.require_empty_attach_check();
            let mut validated = Instant::now()
                .checked_sub(Duration::from_secs(if overdue { 31 } else { 0 }))
                .unwrap();
            if overdue {
                let frame = IncomingFrame::parse(json!({"id":1,
                    "result":{"requiresOpenaiAuth":true,"account":{"type":"chatgpt"}}}));
                let bytes = frame.raw().to_string().len();
                driver.rpc.pending.push_back(frame, bytes);
            }
            let attach = |agent: &str| {
                let RouteSetup {
                    route,
                    output: _output,
                } = Route::for_test(1, Arc::default());
                let (ack, result) = oneshot::channel();
                (Cmd::Attach(agent.into(), route, ack), result)
            };
            let (command, mut result) = attach("next");
            driver.handle_command(command, &mut validated).unwrap();
            result.try_recv().unwrap().unwrap();
            assert!(validated.elapsed() < Duration::from_secs(1));
            assert!(!driver.shard.empty_attach_check());
            // A later admission with active peers must not run synchronous
            // validation, which could consume their acknowledgements.
            validated = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
            let frame = IncomingFrame::parse(json!({"id":99,"result":{}}));
            let bytes = frame.raw().to_string().len();
            driver.rpc.pending.push_back(frame, bytes);
            let (command, mut result) = attach("peer");
            driver.handle_command(command, &mut validated).unwrap();
            result.try_recv().unwrap().unwrap();
            assert_eq!(driver.rpc.pending.len(), 1);
        });
        fixture.finish();
    }
}

#[test]
fn purge_late_thread_start_ack_keeps_native_tracking_and_live_peer_route() {
    use crate::runtime::domain::{AgentState, TaskOutcome};
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::new(launcher.clone());
    with_driver(&fixture, &mut rpc, |driver, _| {
        driver.threads.clear();
        driver.threads.insert("peer-thread".into(), "peer".into());
        let RouteSetup {
            route: peer,
            output:
                RouteOutput {
                    events: _events,
                    failure: failed,
                },
        } = super::Route::for_test(8, std::sync::Arc::default());
        driver.routes.insert("peer".into(), peer);
        let RequestSetup {
            id: request_id,
            mut reply,
        } = request(driver, outgoing::thread_start(None, None), true);
        driver.routes.remove("agent"); // The timed-out/released owner no longer has a route.
        fixture
            .hub
            .blocking_with(|core| {
                let mut a = core.store().agent("agent")?;
                let mut peer = a.clone();
                peer.id = "peer".into();
                peer.thread_id = Some("peer-thread".into());
                core.store().save(&peer)?;
                a.thread_id = None;
                a.turn_id = None;
                a.state = AgentState::Stopped;
                a.task_outcome = TaskOutcome::Cancelled;
                a.conversation_clean = true;
                core.store().save(&a)?;
                core.store().undeliverable("agent")?;
                core.store().purge("agent", false, false, core.running())?;
                Ok(())
            })
            .unwrap();
        route(
            driver,
            IncomingFrame::parse(json!({"id":request_id,"result":{"thread":{"id":"late-thread"}}})),
        )
        .unwrap();
        assert!(reply.try_recv().is_err());
        assert_eq!(
            driver.threads.get("late-thread").map(String::as_str),
            Some("agent")
        );
        assert!(driver.sink_threads.contains("late-thread"));
        assert!(driver.routes.contains_key("peer"));
        assert!(failed.borrow().is_none());
        assert!(!driver.rpc.uncertain());
        fixture
            .hub
            .blocking_with(|c| {
                crate::test_support::assert_code!(c.store().agent("agent"), "agent_purged");
                assert!(c.store().agent("peer").is_ok());
                Ok(())
            })
            .unwrap();
    });
    fixture.finish();
}

fn attach(
    driver: &mut super::ShardLoop<'_>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> tokio::sync::mpsc::Receiver<super::Frame> {
    use super::{Cmd, Route};
    use tokio::sync::oneshot;

    let RouteSetup {
        route,
        output: RouteOutput {
            events: rx,
            failure: _failed,
        },
    } = Route::for_test(1, cancelled);
    let (ack, _ready) = oneshot::channel();
    driver
        .handle_command(
            Cmd::Attach("agent".into(), route, ack),
            &mut std::time::Instant::now(),
        )
        .unwrap();
    rx
}
