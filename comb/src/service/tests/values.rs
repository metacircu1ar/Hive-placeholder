mod admin;
mod authored;
use super::*;

struct InvalidOperation {
    tool: &'static str,
    arguments: Value,
    field: &'static str,
}
impl InvalidOperation {
    fn wire(&self) -> Value {
        let mut operation = self.arguments.clone();
        match self.tool {
            "comb_search" => json!({"op":"search","request":operation}),
            "comb_consolidate" => {
                let fields = operation.as_object_mut().unwrap();
                let request_id = fields.remove("request_id").unwrap();
                let run = fields.remove("run").unwrap();
                json!({"op":"consolidate","request":operation,"request_id":request_id,"run":run})
            }
            _ => {
                operation["op"] = json!(self.tool.strip_prefix("comb_").unwrap());
                operation
            }
        }
    }
    fn assert_error(&self, error: &Error) {
        let input = error.input().expect("typed input error");
        assert_eq!(input.code(), hive_input::Code::InvalidValue);
        assert!(
            input.field().ends_with(&format!("/{}", self.field)),
            "{}",
            input.field()
        );
        assert!(!input.examples().is_empty());
        assert!(error.request_id.is_none(), "{}", self.arguments);
        assert!(!error.file_committed, "{}", self.arguments);
    }
}

fn invalid_put_operations() -> Vec<InvalidOperation> {
    let mut put =
        serde_json::to_value(put("concepts/precedence", "Original", "existing-receipt")).unwrap();
    put.as_object_mut().unwrap().remove("op");
    let mut cases = Vec::new();
    for (field, value) in [
        ("slug", json!("../bad")),
        ("request_id", json!("")),
        ("request_id", json!("x".repeat(129))),
        ("run", json!("\n")),
        ("run", json!("")),
        ("run", json!("x".repeat(513))),
        ("expected_hash", json!("malformed")),
        ("expected_hash", json!("A".repeat(64))),
        ("slug", json!(3)),
        ("request_id", json!(false)),
        ("run", json!([])),
        ("expected_hash", json!({})),
    ] {
        let mut arguments = put.clone();
        arguments[field] = value;
        cases.push(InvalidOperation {
            tool: "comb_put",
            arguments,
            field,
        });
    }
    put["slug"] = json!("../bad");
    put["request_id"] = json!(false);
    cases.push(InvalidOperation {
        tool: "comb_put",
        arguments: put,
        field: "request_id",
    });
    cases
}

fn invalid_operations() -> Vec<InvalidOperation> {
    let mut cases = invalid_put_operations();
    for field in ["note_hash", "target_hash"] {
        let mut arguments = json!({
            "note":"inbox/source", "target":"concepts/precedence",
            "note_hash":"a".repeat(64), "target_hash":"b".repeat(64),
            "summary":"Combined", "request_id":"existing-receipt", "run":"review"
        });
        arguments[field] = json!("malformed");
        cases.push(InvalidOperation {
            tool: "comb_consolidate",
            arguments,
            field,
        });
    }
    cases
}

fn request(connection: &Connection, operation: Value, bad_secret: bool) -> Value {
    let mut credential = serde_json::to_value(&connection.credential).unwrap();
    if bad_secret {
        credential["secret"] = json!("incorrect");
    }
    json!({"protocol":1,"credential":credential,"claim":null,"operation":operation})
}

async fn raw_call(connection: &Connection, request: &Value) -> Error {
    let mut stream = tokio::net::UnixStream::connect(&connection.socket)
        .await
        .unwrap();
    framing::write(&mut stream, request, framing::REQUEST_BYTES)
        .await
        .unwrap();
    let bytes = framing::read(&mut BufReader::new(stream), framing::RESPONSE_BYTES)
        .await
        .unwrap()
        .unwrap();
    serde_json::from_slice::<Reply>(&bytes)
        .unwrap()
        .result()
        .unwrap_err()
}

async fn mcp_call(connection: &Connection, case: &InvalidOperation, bad_secret: bool) -> Error {
    let mut connection = connection.clone();
    if bad_secret {
        connection.credential.secret = "incorrect".into();
    }
    let frames = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}
        }}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":case.tool,"arguments":case.arguments
        }}),
    ];
    let input = frames.map(|frame| format!("{frame}\n")).concat();
    let mut output = Vec::new();
    super::super::mcp::run(connection, input.as_bytes(), &mut output)
        .await
        .unwrap();
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["id"], 2);
    assert_eq!(replies[1]["result"]["isError"], true);
    let content: Value =
        serde_json::from_str(replies[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        content["error"].as_object().unwrap().len(),
        (if content["error"].get("field").is_some() {
            6
        } else {
            3
        }) + usize::from(content["error"].get("request_id").is_some())
    );
    serde_json::from_value(content["error"].clone()).unwrap()
}

fn private_writes(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for directory in ["heads", "receipts"] {
        for entry in std::fs::read_dir(root.join(".comb").join(directory)).unwrap() {
            let path = entry.unwrap().path();
            files.push((path.clone(), std::fs::read(path).unwrap()));
        }
    }
    files.sort();
    files
}

#[test]
fn operation_identifiers_fail_before_authority_reconciliation_and_replay() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let operation = put("concepts/precedence", "Original", "existing-receipt");
        call(&human, &operation, None).await.unwrap();
        let path = dir.path().join("concepts/precedence.md");
        let original = std::fs::read_to_string(&path).unwrap();
        let private = private_writes(dir.path());
        let cases = invalid_operations();
        for expected_file in [original.as_str(), "unparseable external page"] {
            std::fs::write(&path, expected_file).unwrap();
            for case in &cases {
                let hosted = super::super::mcp::operation(
                    case.tool,
                    case.arguments.clone(),
                    Some("hosted-call-id"),
                )
                .err()
                .unwrap();
                case.assert_error(&hosted);
                for bad_secret in [false, true] {
                    let raw = request(&human, case.wire(), bad_secret);
                    case.assert_error(&raw_call(&human, &raw).await);
                    case.assert_error(&mcp_call(&human, case, bad_secret).await);
                }
            }
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected_file);
            assert_eq!(private_writes(dir.path()), private);
            assert!(!dir.path().join(".comb/intent.json").exists());
        }
        let raw = request(&human, serde_json::to_value(operation).unwrap(), true);
        let error = raw_call(&human, &raw).await;
        assert_eq!(error.code, crate::ErrorCode::Forbidden);
        assert_eq!(error.request_id.as_deref(), Some("existing-receipt"));
        running.finish().await;
    });
}

#[test]
fn removal_requires_a_valid_request_id_before_authority_or_target_access() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let created = call(&human, &put("concepts/remove", "Original", "create"), None)
            .await
            .unwrap();
        let path = dir.path().join("concepts/remove.md");
        std::fs::write(&path, "unparseable replacement").unwrap();
        let private = private_writes(dir.path());
        for (id, code) in [
            (None, hive_input::Code::MissingArgument),
            (Some(json!(null)), hive_input::Code::InvalidValue),
            (Some(json!(42)), hive_input::Code::InvalidValue),
            (Some(json!("")), hive_input::Code::InvalidValue),
            (Some(json!("x".repeat(129))), hive_input::Code::InvalidValue),
        ] {
            let mut operation = json!({
                "op":"remove", "slug":"concepts/remove", "expected_hash":created["hash"]
            });
            if let Some(id) = id {
                operation["request_id"] = id;
            }
            assert!(serde_json::from_value::<Operation>(operation.clone()).is_err());
            for bad_secret in [false, true] {
                let error = raw_call(&human, &request(&human, operation.clone(), bad_secret)).await;
                assert_eq!(error.input().unwrap().code(), code);
                assert_eq!(error.input().unwrap().field(), "/operation/request_id");
                assert!(!error.file_committed);
                assert!(error.request_id.is_none());
            }
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "unparseable replacement"
        );
        assert_eq!(private_writes(dir.path()), private);
        assert!(!dir.path().join(".comb/intent.json").exists());
        running.finish().await;
    });
}

#[test]
fn credential_identifier_errors_are_specific_and_precede_authority() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let operation = put("concepts/precedence", "Original", "existing-receipt");
        call(&human, &operation, None).await.unwrap();
        let path = dir.path().join("concepts/precedence.md");
        std::fs::write(&path, "unparseable external page").unwrap();
        let private = private_writes(dir.path());
        for bad_secret in [false, true] {
            for field in ["id", "comb_id"] {
                for (value, code) in [
                    (json!("not-a-uuid"), crate::ErrorCode::InvalidCredential),
                    (json!(42), crate::ErrorCode::InvalidJson),
                ] {
                    let mut raw = request(
                        &human,
                        serde_json::to_value(&operation).unwrap(),
                        bad_secret,
                    );
                    raw["credential"][field] = value;
                    let error = raw_call(&human, &raw).await;
                    assert_eq!(error.code, code);
                    if code == crate::ErrorCode::InvalidCredential {
                        assert_eq!(error.message, Error::invalid_credential_identity().message);
                    }
                    assert!(error.request_id.is_none());
                    assert!(!error.file_committed);
                }
            }
            let raw = request(
                &human,
                json!({"op":"credential_revoke","id":"not-a-uuid"}),
                bad_secret,
            );
            let error = raw_call(&human, &raw).await;
            assert_eq!(error.input().unwrap().field(), "/operation/id");
            assert_eq!(
                error.input().unwrap().code(),
                hive_input::Code::InvalidValue
            );
            assert!(error.request_id.is_none());
            let mut raw = request(
                &human,
                serde_json::to_value(&operation).unwrap(),
                bad_secret,
            );
            raw["credential"]["id"] = json!("not-a-uuid");
            raw["operation"]["request_id"] = json!(false);
            let error = raw_call(&human, &raw).await;
            assert_eq!(error.input().unwrap().field(), "/operation/request_id");
            assert!(error.request_id.is_none());
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "unparseable external page"
        );
        assert_eq!(private_writes(dir.path()), private);
        running.finish().await;
    });
}

#[test]
fn retrieval_rejections_survive_real_socket_and_stdio_mcp() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        for (tool, arguments, operation, field) in [
            (
                "comb_search",
                json!({"query":"q","after":"private-date"}),
                json!({"op":"search","request":{"query":"q","after":"private-date"}}),
                "after",
            ),
            (
                "comb_list",
                json!({"limit":0}),
                json!({"op":"list","request":{"limit":0}}),
                "limit",
            ),
        ] {
            let case = InvalidOperation {
                tool,
                arguments,
                field,
            };
            for bad_secret in [false, true] {
                let socket =
                    raw_call(&human, &request(&human, operation.clone(), bad_secret)).await;
                let mcp = mcp_call(&human, &case, bad_secret).await;
                assert_eq!(
                    socket.input().unwrap().field(),
                    format!("/operation/request/{field}")
                );
                assert_eq!(mcp.input().unwrap().field(), format!("/arguments/{field}"));
                assert_eq!(socket.input().unwrap().code(), mcp.input().unwrap().code());
                assert!(
                    !serde_json::to_string(&socket)
                        .unwrap()
                        .contains("private-date")
                );
                assert!(!socket.file_committed);
                assert!(socket.request_id.is_none());
            }
        }
        running.finish().await;
    });
}
