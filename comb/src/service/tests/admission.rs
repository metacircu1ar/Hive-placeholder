use super::*;
use crate::ErrorCode;

fn request(connection: &Connection, operation: Operation, claim: Option<Claim>) -> Request {
    Request {
        protocol: comb_protocol::compatibility::SERVICE_PROTOCOL,
        credential: connection.credential.clone(),
        operation,
        claim,
    }
}

fn client(state: &State, human: &Connection) -> Connection {
    Connection {
        format: hive_input::compatibility::Exact,
        socket: human.socket.clone(),
        credential: state
            .registry
            .create(Grant::Client {
                name: "Writer".into(),
                reviewer: false,
            })
            .unwrap(),
    }
}

#[test]
fn saturated_writer_precedes_noncollector_review_and_human_denial() {
    let (root, server, human) = setup(None);
    let state = &server.state;
    let client = client(state, &human);
    let path = root.path().join("concepts/forbidden.md");
    runtime().block_on(async {
        let created = state
            .handle(request(
                &human,
                put("concepts/forbidden", "Original", "seed"),
                None,
            ))
            .await
            .unwrap();
        std::fs::write(&path, "unparseable hand edit").unwrap();
        let operations = [
            Operation::Promote {
                slug: "concepts/forbidden".parse().unwrap(),
                expected_hash: created["hash"].as_str().unwrap().parse().unwrap(),
                request_id: "promote".parse().unwrap(),
                run: None,
            },
            Operation::CredentialList,
        ];
        for operation in operations {
            let blocked = state.writes.clone().acquire_many_owned(8).await.unwrap();
            let error = state
                .handle(request(&client, operation.clone(), None))
                .await
                .unwrap_err();
            assert_eq!(error.message, Error::busy("write").message);
            assert!(!error.file_committed);
            drop(blocked);
            let error = state
                .handle(request(&client, operation, None))
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::Forbidden);
            assert!(!error.file_committed);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "unparseable hand edit"
            );
        }
    });
}

#[test]
fn collector_policy_precedes_saturation_but_capabilities_still_bypass_lanes() {
    let (_root, server, human) = setup(None);
    let state = &server.state;
    let bridge = Connection {
        format: hive_input::compatibility::Exact,
        socket: human.socket.clone(),
        credential: state
            .registry
            .create(Grant::Bridge {
                name: "Bridge".into(),
                reviewer_roles: vec![],
            })
            .unwrap(),
    };
    runtime().block_on(async {
        let _blocked = state.writes.clone().acquire_many_owned(8).await.unwrap();
        for operation in [
            Operation::Doctor,
            Operation::CredentialList,
            put("concepts/forbidden", "No", "write"),
        ] {
            let error = state
                .handle(request(&bridge, operation, Some(Claim::Collector)))
                .await
                .unwrap_err();
            assert_eq!(error.message, Error::collector_capture_only().message);
            assert!(!error.file_committed);
        }
        let result = state
            .handle(request(
                &bridge,
                Operation::Capabilities,
                Some(Claim::Collector),
            ))
            .await
            .unwrap();
        assert_eq!(
            result,
            json!({"protocols":{"minimum":1,"maximum":1},"reviewer": false})
        );
        let capture = Operation::Capture {
            text: "Capture".into(),
            request_id: "capture".parse().unwrap(),
            run: None,
        };
        let error = state
            .handle(request(&bridge, capture, Some(Claim::Collector)))
            .await
            .unwrap_err();
        assert_eq!(error.message, Error::busy("write").message);
    });
}

#[test]
fn ordinary_clients_can_run_doctor_but_cannot_list_credentials() {
    let (_root, server, human) = setup(None);
    let state = &server.state;
    let client = client(state, &human);
    runtime().block_on(async {
        let doctor = state
            .handle(request(&client, Operation::Doctor, None))
            .await
            .unwrap();
        assert_eq!(doctor["stats"]["pages"], 0);
        let error = state
            .handle(request(&client, Operation::CredentialList, None))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Forbidden);
        assert!(!error.file_committed);
    });
}

#[test]
fn bridge_health_is_authenticated_read_only_and_avoids_saturated_writer() {
    let (_root, server, human) = setup(None);
    let state = &server.state;
    let bridge = Connection {
        format: hive_input::compatibility::Exact,
        socket: human.socket.clone(),
        credential: state
            .registry
            .create(Grant::Bridge {
                name: "Diagnostics".into(),
                reviewer_roles: vec![],
            })
            .unwrap(),
    };
    runtime().block_on(async {
        let _blocked = state.writes.clone().acquire_many_owned(8).await.unwrap();
        let health = state
            .handle(request(&bridge, Operation::Health, Some(Claim::Collector)))
            .await
            .unwrap();
        assert_eq!(health["total"], 0);
        assert_eq!(health["model"], Value::Null);
        assert_eq!(health["product_version"], env!("CARGO_PKG_VERSION"));
        assert!(health.get("pages").is_none());
        assert!(
            state
                .handle(request(&bridge, Operation::Health, None))
                .await
                .is_err()
        );
        let mut forged = bridge.clone();
        forged.credential.secret = "0".repeat(64);
        assert!(
            state
                .handle(request(&forged, Operation::Health, Some(Claim::Collector)))
                .await
                .is_err()
        );
        assert!(
            state
                .handle(request(&bridge, Operation::Stats, Some(Claim::Collector)))
                .await
                .is_err()
        );
    });
}

#[test]
fn snapshot_failure_precedes_search_validation_and_query_failure() {
    for mode in [Mode::Vector, Mode::Hybrid] {
        let (_root, server, human) = setup(None);
        let operation = Operation::Search {
            request: Search {
                query: "missing model".into(),
                limit: 0,
                ..Search::default()
            },
            mode,
        };
        runtime().block_on(async {
            let error = server
                .state
                .handle(request(&human, operation.clone(), None))
                .await
                .unwrap_err();
            assert_eq!(error.message, Error::invalid_search().message);
            assert_eq!(
                server.state.reads.available_permits(),
                super::super::limits::READ_SLOTS
            );
            server.state.publication.fail();
            let error = server
                .state
                .handle(request(&human, operation, None))
                .await
                .unwrap_err();
            assert_eq!(error.message, Error::pending_state().message);
            assert_eq!(error.code, ErrorCode::RecoveryRequired);
            assert_eq!(
                server.state.reads.available_permits(),
                super::super::limits::READ_SLOTS
            );
        });
    }
}
