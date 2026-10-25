//! Provider routing, attachment isolation and owner-failure requirements.
use super::corpus_support::{RequestSetup, request, route, with_driver};
use crate::runtime::protocol_corpus_support::{ExpectedRoute, frames};
use crate::runtime::provider::RpcHarness;
use crate::runtime::provider::protocol::IncomingFrame;
use crate::runtime::provider::protocol::outgoing;
use crate::runtime::test_support::RuntimeFixture;
use serde_json::json;
use std::sync::atomic::Ordering;

#[test]
fn protocol_corpus_shard_routes_fixture_and_mutated_frames_without_rebuilding() {
    let launcher = crate::launch::test_cache().unwrap();
    for case in frames()
        .into_iter()
        .filter(|case| case.route != ExpectedRoute::Response)
    {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::active(launcher.clone());
        let raw = rpc.ingest(&case.frame).unwrap();
        with_driver(&fixture, &mut rpc, |driver, output| {
            let result = route(driver, IncomingFrame::parse(raw));
            assert_eq!(
                result.as_ref().err().map(crate::error::Error::code),
                (case.route == ExpectedRoute::GlobalFailure).then_some("provider_protocol_error"),
                "{}",
                case.name
            );
            let delivered = output.received();
            assert_eq!(
                delivered,
                (case.route == ExpectedRoute::Owner).then(|| case.frame.clone()),
                "{}",
                case.name
            );
            assert_eq!(
                output
                    .failure
                    .borrow()
                    .as_ref()
                    .map(crate::error::Error::code),
                (case.route == ExpectedRoute::OwnerFailure).then_some("provider_protocol_error"),
                "{}",
                case.name
            );
            assert_eq!(
                driver.shard.draining(),
                case.route == ExpectedRoute::OwnerFailure,
                "{}",
                case.name
            );
            assert_eq!(
                driver.sink_threads.contains("thread"),
                case.route == ExpectedRoute::OwnerFailure,
                "{}",
                case.name
            );
            if let ExpectedRoute::Notice(notice) = case.route {
                assert!(driver.notices.contains(notice.as_str()), "{}", case.name);
            }
        });
        if case.route == ExpectedRoute::Unsupported {
            assert_eq!(
                rpc.outgoing(3)[2],
                json!({"id":case.frame["id"],"error":{
                "code":-32601,"message":"Unsupported provider request."}}),
                "{}",
                case.name
            );
        }
        let events = fixture.events();
        assert_eq!(
            events["events"].as_array().unwrap().len(),
            usize::from(case.route == ExpectedRoute::OwnerFailure),
            "{}",
            case.name
        );
        fixture.finish();
    }
}

#[test]
fn protocol_corpus_shard_response_presence_and_invalid_ids() {
    let launcher = crate::launch::test_cache().unwrap();
    for id in [
        None,
        Some(json!(null)),
        Some(json!("3")),
        Some(json!(3.0)),
        Some(json!(-1)),
        Some(json!(false)),
        Some(json!(999)),
    ] {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::active(launcher.clone());
        with_driver(&fixture, &mut rpc, |driver, output| {
            let mut frame = json!({"result":null});
            if let Some(id) = id {
                frame["id"] = id;
            }
            crate::test_support::assert_code!(
                route(driver, IncomingFrame::parse(frame)),
                "provider_protocol_error"
            );
            assert!(output.received().is_none());
            assert!(output.failure.borrow().is_none());
            assert!(!driver.rpc.uncertain());
        });
        fixture.finish();
    }
    for malformed in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::active(launcher.clone());
        with_driver(&fixture, &mut rpc, |driver, output| {
            let RequestSetup { id, mut reply } =
                request(driver, outgoing::terminal_list("thread"), false);
            let frame = if malformed {
                json!({"id":id,"result":null,"error":null})
            } else {
                json!({"id":id,"error":{"code":-1,"message":"rejected"}})
            };
            route(driver, IncomingFrame::parse(frame)).unwrap();
            assert_eq!(
                reply.try_recv().unwrap().err().unwrap().code(),
                if malformed {
                    "provider_protocol_error"
                } else {
                    "provider_request_failed"
                }
            );
            assert_eq!(output.failure.borrow().is_some(), malformed);
            assert_eq!(driver.routes.contains_key("agent"), !malformed);
            assert_eq!(driver.rpc.thread_uncertain("thread"), malformed);
            assert!(!driver.rpc.uncertain());
        });
        fixture.finish();
    }
}

#[test]
fn protocol_corpus_shard_binding_ack_and_late_tombstone_preserve_ownership() {
    let launcher = crate::launch::test_cache().unwrap();
    for tombstone in [false, true] {
        let fixture = RuntimeFixture::new();
        fixture
            .hub
            .blocking_with(|core| {
                let mut agent = core.store().agent("agent")?;
                agent.thread_id = None;
                core.store().save(&agent)
            })
            .unwrap();
        let mut rpc = RpcHarness::new(launcher.clone());
        with_driver(&fixture, &mut rpc, |driver, _| {
            driver.threads.clear();
            let RequestSetup { id, mut reply } =
                request(driver, outgoing::thread_start(None, None), tombstone);
            let result = json!({"thread":{"id":"new-thread","extra":true},"opaque":[null,1]});
            route(
                driver,
                IncomingFrame::parse(json!({"id":id,"result":result})),
            )
            .unwrap();
            assert_eq!(
                driver.threads.get("new-thread").map(String::as_str),
                Some("agent")
            );
            assert_eq!(driver.sink_threads.contains("new-thread"), tombstone);
            if tombstone {
                assert!(reply.try_recv().is_err());
            } else {
                assert_eq!(reply.try_recv().unwrap().unwrap().into_raw(), result);
            }
        });
        let events = fixture.events();
        assert_eq!(
            events["events"].as_array().unwrap().len(),
            usize::from(tombstone)
        );
        if tombstone {
            assert_eq!(events["events"][0]["type"], "thread_bound");
            assert_eq!(
                events["events"][0]["data"],
                json!({"thread_id":"new-thread","late_ack":true})
            );
        }
        fixture.finish();
    }
    for same_owner in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::new(launcher.clone());
        with_driver(&fixture, &mut rpc, |driver, _| {
            if !same_owner {
                driver.threads.insert("new-thread".into(), "sibling".into());
            }
            let RequestSetup { id, .. } =
                request(driver, outgoing::thread_start(None, None), false);
            crate::test_support::assert_code!(
                route(
                    driver,
                    IncomingFrame::parse(json!({"id":id,"result":{"thread":{"id":"new-thread"}}}))
                ),
                "provider_protocol_error"
            );
            assert!(driver.rpc.uncertain());
        });
        fixture.finish();
    }
}

#[test]
fn protocol_corpus_shard_notice_dedup_redacts_payload_and_fails_only_owned_route() {
    let launcher = crate::launch::test_cache().unwrap();
    let fixture = RuntimeFixture::new();
    let mut rpc = RpcHarness::active(launcher.clone());
    with_driver(&fixture, &mut rpc, |driver, output| {
        for method in ["thread/started", "thread/status/changed", "thread/closed"] {
            route(
                driver,
                IncomingFrame::parse(
                    json!({"method":method,"params":{"credential":"must not persist"}}),
                ),
            )
            .unwrap();
        }
        assert_eq!(driver.notices.len(), 1);
        driver.rpc.quarantine("thread");
        let request = json!({"id":null,"method":"item/tool/call","params":{
            "threadId":"thread","turnId":"turn","arguments":{"private":true}}});
        route(driver, IncomingFrame::parse(request)).unwrap();
        assert_eq!(
            output.failure.borrow().as_ref().unwrap().code(),
            "provider_protocol_error"
        );
        assert!(!driver.routes.contains_key("agent"));
        assert!(!driver.rpc.uncertain());
    });
    assert_eq!(
        rpc.outgoing(3)[2],
        json!({"id":null,"error":{
        "code":-32601,"message":"Session is not accepting tool requests."}})
    );
    let connection =
        rusqlite::Connection::open(fixture.temp.path().join("runtime.sqlite")).unwrap();
    let rows: Vec<(String, String)> = connection
        .prepare("SELECT kind,data FROM provider_events")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![(
            "thread_metadata".into(),
            json!({"scope":"connection","details":"redacted"}).to_string()
        )]
    );
    drop(connection);
    fixture.finish();
}

#[test]
fn failed_delivery_preserves_opaque_request_ids_and_releases_frame_charge() {
    let launcher = crate::launch::test_cache().unwrap();
    for id in [
        json!("opaque"),
        json!(17),
        json!(null),
        json!({"opaque":[1,2]}),
    ] {
        let fixture = RuntimeFixture::new();
        let mut rpc = RpcHarness::active(launcher.clone());
        with_driver(&fixture, &mut rpc, |driver, output| {
            output.events.close();
            let frame = json!({"id":id,"method":"item/tool/call","params":{
                "threadId":"thread","turnId":"turn","callId":"call",
                "tool":"hive_agents","arguments":{}}});
            driver.handle_frame(IncomingFrame::parse(frame)).unwrap();
            assert!(driver.sink_threads.contains("thread"));
            assert!(driver.shard.draining());
            assert!(!driver.routes.contains_key("agent"));
            assert!(matches!(
                *output.failure.borrow(),
                Some(crate::error::Error::OutputLimit { .. })
            ));
            assert_eq!(driver.bytes.load(Ordering::Relaxed), 0);
        });
        assert_eq!(
            rpc.outgoing(3)[2],
            json!({"id":id,"error":{
            "code":-32601,"message":"Session is not accepting tool requests."}})
        );
        fixture.finish();
    }
}
