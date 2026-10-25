#[test]
fn account_probe_request_bytes_remain_exact() {
    assert_eq!(
        super::initialize().to_string(),
        format!(
            "{{\"id\":1,\"method\":\"initialize\",\"params\":{{\"clientInfo\":{{\"name\":\"hive\",\"version\":\"{}\"}}}}}}",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(
        super::initialized().to_string(),
        r#"{"method":"initialized"}"#
    );
    assert_eq!(
        super::account_read().to_string(),
        r#"{"id":2,"method":"account/read","params":{"refreshToken":false}}"#
    );
}
