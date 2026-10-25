use super::*;

#[test]
fn administrative_rejections_preserve_authority_order_and_leave_storage_unchanged() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let client = issue(
            &human,
            Grant::Client {
                name: "client".into(),
                reviewer: true,
            },
        )
        .await;
        call(&human, &put("concepts/admin", "Original", "original"), None)
            .await
            .unwrap();
        let registry = dir.path().join(".comb/credentials.json");
        let credentials = std::fs::read(&registry).unwrap();
        let page = dir.path().join("concepts/admin.md");
        let contents = std::fs::read(&page).unwrap();
        let private = private_writes(dir.path());
        for connection in [&human, &client] {
            for bad_secret in [false, true] {
                for (operation, field) in [
                    (
                        json!({"op":"import","directory":"/private/missing","prefix":"../bad"}),
                        "/operation/prefix",
                    ),
                    (
                        json!({"op":"credential_revoke","id":"private"}),
                        "/operation/id",
                    ),
                    (
                        json!({"op":"credential_create","grant":{"kind":"client","name":false}}),
                        "/operation/grant/name",
                    ),
                ] {
                    let error =
                        raw_call(connection, &request(connection, operation, bad_secret)).await;
                    assert_eq!(error.input().unwrap().field(), field);
                    assert!(!error.file_committed);
                }
                for grant in [
                    json!({"kind":"client","name":""}),
                    json!({"kind":"bridge","name":"bridge","reviewer_roles":["same","same"]}),
                ] {
                    let raw = request(
                        connection,
                        json!({"op":"credential_create","grant":grant}),
                        bad_secret,
                    );
                    let error = raw_call(connection, &raw).await;
                    if bad_secret || connection.credential.id != human.credential.id {
                        assert_eq!(error.code, crate::ErrorCode::Forbidden);
                        assert!(error.input().is_none());
                    } else {
                        assert!(
                            error
                                .input()
                                .unwrap()
                                .field()
                                .starts_with("/operation/grant/")
                        );
                    }
                    assert!(!error.file_committed);
                    assert!(error.request_id.is_none());
                }
            }
        }
        assert_eq!(std::fs::read(registry).unwrap(), credentials);
        assert_eq!(std::fs::read(page).unwrap(), contents);
        assert_eq!(private_writes(dir.path()), private);
        assert!(!dir.path().join(".comb/intent.json").exists());
        running.finish().await;
    });
}

#[test]
fn mcp_discovery_distinguishes_bad_revoked_bridge_and_valid_credentials() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let reviewer = issue(
            &human,
            Grant::Client {
                name: "reviewer".into(),
                reviewer: true,
            },
        )
        .await;
        let reader = issue(
            &human,
            Grant::Client {
                name: "reader".into(),
                reviewer: false,
            },
        )
        .await;
        let bridge = issue(
            &human,
            Grant::Bridge {
                name: "bridge".into(),
                reviewer_roles: vec![],
            },
        )
        .await;
        for (connection, reviewer) in [
            (human.clone(), true),
            (reviewer.clone(), true),
            (reader, false),
        ] {
            let replies = discover(connection).await;
            let tools = replies[1]["result"]["tools"].as_array().unwrap();
            assert_eq!(tools.iter().any(|t| t["name"] == "comb_promote"), reviewer);
            assert!(replies[2]["result"].is_object());
        }
        call(
            &human,
            &Operation::CredentialRevoke {
                id: reviewer.credential.id.clone(),
            },
            None,
        )
        .await
        .unwrap();
        let mut wrong = human.clone();
        wrong.credential.secret = "private-secret".into();
        for connection in [wrong, reviewer, bridge] {
            let replies = discover(connection).await;
            assert_eq!(replies[1]["error"]["code"], -32000);
            assert!(
                replies[1]["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("rejected the credential")
            );
            assert!(!replies[1].to_string().contains("private-secret"));
            assert!(replies[2]["result"].is_object());
        }
        running.finish().await;
    });
}
async fn discover(connection: Connection) -> Vec<Value> {
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}
        }}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
    ];
    let input = frames.map(|frame| format!("{frame}\n")).concat();
    let mut output = Vec::new();
    super::super::super::mcp::run(connection, input.as_bytes(), &mut output)
        .await
        .unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
