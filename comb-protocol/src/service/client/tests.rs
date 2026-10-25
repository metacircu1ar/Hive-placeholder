use super::*;
use crate::{ErrorCode, service::auth::Credential, values::CombId};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

async fn exchange(reply: Value) -> Error {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.path().canonicalize().unwrap().join("comb.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket,
        credential: Credential {
            comb_id: CombId::generate(),
            id: "human".to_owned().try_into().unwrap(),
            secret: "fixture-secret".into(),
        },
    };
    let operation = Operation::Capture {
        text: "fixture note".into(),
        request_id: "capture-fixture".to_owned().try_into().unwrap(),
        run: None,
    };
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let bytes = framing::read(&mut stream, framing::REQUEST_BYTES)
            .await
            .unwrap()
            .unwrap();
        let request = super::super::input::request(&bytes).unwrap();
        assert_eq!(request.protocol, 1);
        assert!(matches!(request.operation, Operation::Capture { .. }));
        framing::write(stream.get_mut(), &reply, framing::RESPONSE_BYTES)
            .await
            .unwrap();
    };
    let (error, ()) = tokio::join!(
        call(&connection, &operation, None, Duration::from_secs(2)),
        server
    );
    error.unwrap_err()
}
fn future_error(committed: bool) -> Value {
    json!({"protocol":1,"status":"error","error":{
        "code":"future_failure","message":"Future peer error.",
        "file_committed":committed,"request_id":"capture-fixture",
        "import_progress":{"batch_id":"00000000-0000-4000-8000-000000000001",
            "state":"pending","total":3,"completed":1,"unfinished":2,"unresolved":"notes/one"}
    }})
}
#[tokio::test]
async fn unknown_errors_preserve_acceptance_identity_and_progress() {
    for committed in [false, true] {
        let error = exchange(future_error(committed)).await;
        assert_eq!(error.code, ErrorCode::UnknownError);
        assert_eq!(error.raw_code(), Some("future_failure"));
        let encoded = serde_json::to_value(&error).unwrap();
        assert_eq!(encoded["code"], "unknown_error");
        assert_eq!(encoded["raw_code"], "future_failure");
        let roundtrip: Error = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(roundtrip.raw_code(), error.raw_code());
        assert_eq!(serde_json::to_value(roundtrip).unwrap(), encoded);
        assert_eq!(error.file_committed, committed);
        assert_eq!(error.request_id.as_deref(), Some("capture-fixture"));
        let progress = error.import_progress.unwrap();
        assert_eq!(progress.total, 3);
        assert_eq!(progress.completed, 1);
        assert_eq!(progress.unfinished, 2);
    }
}
#[tokio::test]
async fn malformed_replies_after_submission_remain_uncertain_without_retry() {
    let original = future_error(false);
    let mut cases = Vec::new();
    for (key, value) in [("protocol", json!(2)), ("extra", json!(true))] {
        let mut reply = original.clone();
        reply[key] = value;
        cases.push(reply);
    }
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("protocol");
    cases.push(missing);
    for (key, value) in [
        ("file_committed", json!("false")),
        ("request_id", json!("")),
        ("code", json!("invalid code")),
        ("import_progress", json!(null)),
    ] {
        let mut reply = original.clone();
        reply["error"][key] = value;
        cases.push(reply);
    }
    let mut progress = original;
    progress["error"]["import_progress"]["unfinished"] = json!(0);
    cases.push(progress);
    for reply in cases {
        let error = exchange(reply).await;
        assert_eq!(error.code, ErrorCode::Uncertain, "{error:?}");
        assert!(error.file_committed);
        assert_eq!(error.request_id.as_deref(), Some("capture-fixture"));
    }
}

#[test]
fn compatibility_service_contract() {
    let original: Value =
        serde_json::from_str(include_str!("../../compatibility/service-envelope.json")).unwrap();
    let request =
        super::super::input::request(&serde_json::to_vec(&original["request"]).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), original["request"]);
    assert_eq!(
        serde_json::to_value(Reply::from_result(Ok(json!({"ready":true})))).unwrap(),
        original["reply"]
    );
    for protocol in [None, Some(json!(2)), Some(json!(null))] {
        let mut value = original["request"].clone();
        value.as_object_mut().unwrap().remove("protocol");
        if let Some(protocol) = protocol {
            value["protocol"] = protocol;
        }
        assert!(super::super::input::request(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let positional = json!([1,original["request"]["credential"],null,{"op":"health"}]);
    assert!(super::super::input::request(&serde_json::to_vec(&positional).unwrap()).is_err());
}
