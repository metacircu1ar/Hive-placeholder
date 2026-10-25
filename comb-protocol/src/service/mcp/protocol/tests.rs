use super::*;
use crate::error::value;
use serde_json::json;

fn invalid(bytes: &[u8], expected: Value) {
    let Err(error) = decode(bytes) else {
        panic!("invalid frame accepted");
    };
    assert_eq!(value(error).unwrap(), expected);
}

#[test]
fn duplicate_envelope_keys_keep_the_last_value_before_validation() {
    let bytes = br#"{"jsonrpc":"wrong","jsonrpc":"2.0","id":null,"id":18446744073709551615,"method":false,"method":"ping"}"#;
    let Ok(Message::Request { id, method, .. }) = decode(bytes) else {
        panic!("last valid values must win");
    };
    assert_eq!(method, "ping");
    assert_eq!(value(id).unwrap(), json!(u64::MAX));
    invalid(
        br#"{"jsonrpc":"2.0","id":1,"id":null,"method":"ping"}"#,
        json!({"code":-32600,"message":"Invalid JSON-RPC request."}),
    );
}

#[test]
fn object_ids_require_strings_or_signed_or_unsigned_integers() {
    for id in [
        json!(""),
        json!("request"),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(0),
    ] {
        let frame = json!({"jsonrpc":"2.0","id":id,"method":"ping","vendor":true});
        let Ok(Message::Request { id: actual, .. }) = decode(frame.to_string().as_bytes()) else {
            panic!("valid ID rejected");
        };
        assert_eq!(value(actual).unwrap(), id);
    }
    for id in [Value::Null, json!(1.0), json!(true), json!([]), json!({})] {
        let frame = json!({"jsonrpc":"2.0","id":id,"method":"ping"});
        invalid(
            frame.to_string().as_bytes(),
            json!({"code":-32600,"message":"Invalid JSON-RPC request."}),
        );
    }
}

#[test]
fn absent_ids_are_notifications_but_still_require_a_valid_envelope() {
    let Ok(Message::Notification { method }) =
        decode(br#"{"jsonrpc":"2.0","method":"vendor/notice","params":null,"extra":1}"#)
    else {
        panic!("valid notification rejected");
    };
    assert_eq!(method, "vendor/notice");
    for bytes in [
        br#"{"jsonrpc":"2.0"}"#.as_slice(),
        b"[]",
        br#"{"jsonrpc":"2.0","method":false}"#,
    ] {
        invalid(
            bytes,
            json!({"code":-32600,"message":"Invalid JSON-RPC request."}),
        );
    }
    invalid(b"{", json!({"code":-32700,"message":"Invalid JSON."}));
}

#[test]
fn positional_frames_preserve_opaque_ids_and_null_notifications() {
    for id in [
        json!(1.5),
        json!(true),
        json!({"opaque":1}),
        json!([1]),
        json!("id"),
        json!(u64::MAX),
    ] {
        let frame = json!(["2.0", id, "ping", null]);
        let Ok(Message::Request {
            id: actual, method, ..
        }) = decode(frame.to_string().as_bytes())
        else {
            panic!("positional request rejected");
        };
        assert_eq!(value(actual).unwrap(), id);
        assert_eq!(method, "ping");
    }
    let Ok(Message::Notification { method }) =
        decode(br#"["2.0",null,"notifications/initialized",null]"#)
    else {
        panic!("positional notification rejected");
    };
    assert_eq!(method, "notifications/initialized");
    invalid(
        br#"["2.0",1,"ping",null,"extra"]"#,
        json!({"code":-32600,"message":"Invalid JSON-RPC request."}),
    );
}
