use super::*;
use crate::golden;

#[test]
fn request_frames_are_byte_identical() {
    let operations = json!([
        {"op":"capabilities"}, {"op":"health"}, {"op":"get","slug":"concepts/page"},
        {"op":"search","request":{"query":"writer"}}, {"op":"list","request":{}},
        {"op":"links","slug":"concepts/page"}, {"op":"graph","slug":"concepts/page","depth":2},
        {"op":"stats"}, {"op":"doctor"},
        {"op":"put","slug":"concepts/page","markdown":"---\ntitle: Page\n---\nText","request_id":"put"},
        {"op":"append","slug":"concepts/page","text":"Evidence","source":"test","request_id":"append"},
        {"op":"capture","text":"Note","request_id":"capture"},
        {"op":"promote","slug":"concepts/page","expected_hash":"ab".repeat(32),"request_id":"promote"},
        {"op":"consolidate","request":{"note":"inbox/note","target":"concepts/page","note_hash":"ab".repeat(32),"target_hash":"cd".repeat(32),"summary":"Summary [evidence:new]","recopy":false},"request_id":"consolidate","run":"run"},
        {"op":"decide","slug":"review/page","expected_hash":"ab".repeat(32),"decision":"reject","reason":"Missing evidence","request_id":"decide"},
        {"op":"remove","slug":"concepts/page","expected_hash":"ab".repeat(32),"request_id":"remove"},
        {"op":"sync"}, {"op":"import","directory":"/tmp/import","prefix":"concepts"},
        {"op":"import_status"}, {"op":"import_abandon","batch_id":golden::ID},
        {"op":"credential_create","grant":{"kind":"client","name":"Writer","reviewer":false}},
        {"op":"credential_list"}, {"op":"credential_revoke","id":golden::ID}
    ]);
    let credential = auth::Credential {
        comb_id: (uuid::Uuid::parse_str(golden::ID).unwrap()).into(),
        id: golden::ID.parse().unwrap(),
        secret: "ab".repeat(32),
    };
    let mut frames = Vec::new();
    for operation in operations.as_array().unwrap() {
        let request = api::Request {
            protocol: comb_protocol::compatibility::SERVICE_PROTOCOL,
            credential: credential.clone(),
            claim: Some(claim("agent", "writer")),
            operation: serde_json::from_value(operation.clone()).unwrap(),
        };
        let bytes = framing::encode(&request, framing::REQUEST_BYTES).unwrap();
        let restored = input::request(&bytes).unwrap();
        assert_eq!(
            bytes,
            framing::encode(&restored, framing::REQUEST_BYTES).unwrap()
        );
        frames.extend(bytes);
    }
    golden::check("socket-requests.jsonl", frames);
}

#[test]
fn reply_frames_are_byte_identical() {
    let mut frames = Vec::new();
    for result in [
        Ok(json!({"text":"quoted \"text\"\n日本語"})),
        Err(Error::not_found()),
        Err(import_error()),
        Err(put("concepts/page", "Text", "put").lost_reply()),
        Err(Operation::Stats.lost_reply()),
        Err(Operation::Remove {
            slug: "concepts/page".parse().unwrap(),
            expected_hash: ("ab".repeat(32)).parse().unwrap(),
            request_id: "remove".parse().unwrap(),
        }
        .lost_reply()),
        Ok(crate::error::value(crate::store::Removed {
            removed: "concepts/page".parse().unwrap(),
            request_id: "remove".parse().unwrap(),
            replayed: false,
        })
        .unwrap()),
        Ok(crate::error::value(crate::store::Removed {
            removed: "concepts/page".parse().unwrap(),
            request_id: "remove".parse().unwrap(),
            replayed: true,
        })
        .unwrap()),
    ] {
        let bytes =
            framing::encode(&api::Reply::from_result(result), framing::RESPONSE_BYTES).unwrap();
        let restored: api::Reply = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            bytes,
            framing::encode(&restored, framing::RESPONSE_BYTES).unwrap()
        );
        frames.extend(bytes);
    }
    golden::check("socket-replies.jsonl", frames);
}

#[test]
fn mcp_catalogs_are_byte_identical() {
    for reviewer in [false, true] {
        golden::json(
            &format!("mcp-catalog-{reviewer}.json"),
            &super::super::mcp::tools(reviewer),
        );
    }
}

#[test]
fn actual_mcp_response_frames_pin_session_errors_and_tool_output() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let (stop,stopped) = tokio::sync::oneshot::channel();
        let running = tokio::spawn(server.run(async { let _ = stopped.await; }));
        let input = [
            "{broken".to_owned(),
            json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}).to_string(),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}}}).to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            json!({"jsonrpc":"2.0","id":"ping","method":"ping"}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"comb_list","arguments":{}}}).to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"concepts/missing"}}}).to_string(),
            json!({"jsonrpc":"2.0","id":4,"method":"unknown"}).to_string(),
            json!({"jsonrpc":"2.0","id":null,"method":"ping"}).to_string(),
        ].join("\n")+"\n";
        let mut output=Vec::new();
        super::super::mcp::run(human, input.as_bytes(), &mut output).await.unwrap();
        golden::check("mcp-responses.jsonl",output);
        stop.send(()).unwrap();
        running.await.unwrap().unwrap();
    });
}

fn import_error() -> Error {
    let mut error = Error::io(std::io::Error::other("private filesystem cause")).committed();
    error.import_progress = Some(Box::new(comb_protocol::import::ImportProgress {
        batch_id: golden::ID.parse().unwrap(),
        state: comb_protocol::import::ImportState::Pending,
        total: 3,
        completed: 1,
        unfinished: 2,
        unresolved: Some("concepts/page".parse().unwrap()),
    }));
    error
}
