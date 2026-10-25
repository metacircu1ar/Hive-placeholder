use super::*;

async fn bridge(human: &Connection) -> Connection {
    issue(
        human,
        Grant::Bridge {
            name: "runtime".into(),
            reviewer_roles: vec!["reviewer".into()],
        },
    )
    .await
}

async fn client(human: &Connection) -> Connection {
    issue(
        human,
        Grant::Client {
            name: "outside".into(),
            reviewer: false,
        },
    )
    .await
}

fn promote(created: &Value) -> Operation {
    Operation::Promote {
        slug: "components/writer".parse().unwrap(),
        expected_hash: created["hash"].as_str().unwrap().parse().unwrap(),
        request_id: "review".parse().unwrap(),
        run: None,
    }
}

fn append() -> Operation {
    Operation::Append {
        slug: "components/writer".parse().unwrap(),
        text: "Disk sync measured".into(),
        source: "test:measurement".into(),
        summary: None,
        expected_hash: None,
        request_id: "lost-reply".parse().unwrap(),
        run: Some("review-run".parse().unwrap()),
    }
}

async fn lose_reply(bridge: &Connection, append: &Operation) {
    let mut socket = UnixStream::connect(&bridge.socket).await.unwrap();
    socket
        .write_all(
            &framing::encode(
                &Request {
                    protocol: comb_protocol::compatibility::SERVICE_PROTOCOL,
                    credential: bridge.credential.clone(),
                    claim: Some(claim("a", "implementor")),
                    operation: append.clone(),
                },
                framing::REQUEST_BYTES,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    drop(socket);
}

fn reopen(root: &std::path::Path, human: &Connection) -> Server {
    Server::open(
        root,
        Options {
            socket: Some(human.socket.clone()),
            ..Options::default()
        },
    )
    .unwrap()
}

#[test]
fn socket_attributes_bridge_writes_and_enforces_reviewer_capabilities() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let bridge = bridge(&human).await;
        let client = client(&human).await;
        let created = call(
            &bridge,
            &put("components/writer", "Original summary", "create"),
            Some(claim("a", "implementor")),
        )
        .await
        .unwrap();
        let doc = get(&client, "components/writer").await;
        assert_eq!(doc["page"]["metadata"]["author"]["id"], "a");
        assert_eq!(doc["unverified"], true);
        assert_eq!(
            call(
                &client,
                &Operation::Stats,
                Some(claim("forged", "reviewer"))
            )
            .await
            .unwrap_err()
            .code,
            crate::ErrorCode::Forbidden
        );
        assert_eq!(
            call(
                &bridge,
                &Operation::CredentialList,
                Some(claim("a", "implementor"))
            )
            .await
            .unwrap_err()
            .code,
            crate::ErrorCode::Forbidden
        );
        let promote = promote(&created);
        assert_eq!(
            call(&bridge, &promote, Some(claim("a", "implementor")))
                .await
                .unwrap_err()
                .code,
            crate::ErrorCode::Forbidden
        );
        call(&bridge, &promote, Some(claim("b", "reviewer")))
            .await
            .unwrap();
        running.finish().await;
    });
}

#[test]
fn socket_client_attribution_and_revocation_survive_restart() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let client = client(&human).await;
        call(
            &client,
            &put("concepts/client", "Client authored", "client-create"),
            None,
        )
        .await
        .unwrap();
        let doc = get(&human, "concepts/client").await;
        assert_eq!(doc["page"]["metadata"]["author"]["kind"], "client");
        assert_eq!(doc["unverified"], true);
        call(
            &human,
            &Operation::CredentialRevoke {
                id: client.credential.id.clone(),
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            call(&client, &Operation::Stats, None)
                .await
                .unwrap_err()
                .code,
            crate::ErrorCode::Forbidden
        );
        running.finish().await;
        let running = Running::start(reopen(dir.path(), &human));
        assert_eq!(
            call(&client, &Operation::Stats, None)
                .await
                .unwrap_err()
                .code,
            crate::ErrorCode::Forbidden
        );
        running.finish().await;
    });
}

#[test]
fn socket_lost_ack_retries_once_and_replays_after_restart() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let bridge = bridge(&human).await;
        let created = call(
            &bridge,
            &put("components/writer", "Original summary", "create"),
            Some(claim("a", "implementor")),
        )
        .await
        .unwrap();
        call(&bridge, &promote(&created), Some(claim("b", "reviewer")))
            .await
            .unwrap();
        let append = append();
        lose_reply(&bridge, &append).await;
        // A retry races the original accepted request. Serialization + durable
        // receipt permits exactly one append whichever connection wins.
        let first = call(&bridge, &append, Some(claim("a", "implementor")))
            .await
            .unwrap();
        let again = call(&bridge, &append, Some(claim("a", "implementor")))
            .await
            .unwrap();
        assert!(first["request_id"].is_string());
        assert_eq!(again["replayed"], true);
        let doc = get(&human, "components/writer").await;
        assert_eq!(doc["page"]["timeline"].as_array().unwrap().len(), 1);
        assert_eq!(doc["page"]["timeline"][0]["metadata"]["actor"]["id"], "a");
        running.finish().await;
        let running = Running::start(reopen(dir.path(), &human));
        assert_eq!(
            call(&bridge, &append, Some(claim("a", "implementor")))
                .await
                .unwrap()["replayed"],
            true
        );
        running.finish().await;
    });
}
