//! Streaming regressions.
use super::*;

#[test]
fn blocking_tools_are_never_advertised_or_accepted_inside_agents() {
    for name in ["hive_call", "hive_wait_message"] {
        assert!(
            !tools()
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name)
        );
        assert!(validate_tool(name, &json!({})).is_err());
    }
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    assert_eq!(
        call(
            &mut core,
            "internal",
            "read",
            json!({"op":"message_result","message_id":"x"})
        )
        .unwrap_err()
        .code(),
        "unsupported_operation"
    );
}
