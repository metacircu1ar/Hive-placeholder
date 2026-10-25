//! Indexing regressions.
use super::*;

#[test]
fn assistant_text_blocks_are_combined_in_native_order_with_model_provenance() {
    let mut f = Fixture::new();
    f.turn();
    f.codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "model":"claude-sonnet-5","content":[{"type":"text","text":"first"},
        {"type":"thinking","thinking":"not output"},{"type":"text","text":"second"}]}}))
        .unwrap();
    for text in ["first", "second", "first\nsecond"] {
        let frame = f.codec.pop().unwrap();
        assert_eq!(frame["params"]["item"]["text"], text);
        assert_eq!(frame["params"]["item"]["verifiedModel"], true);
    }
    assert!(f.codec.pop().is_none());
    f.codec
        .receive(json!({"type":"assistant","session_id":f.id,"message":{
        "model":"<synthetic>","content":[{"type":"text","text":"local notice"}]}}))
        .unwrap();
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["verifiedModel"],
        false
    );
}

#[test]
fn models_without_effort_use_none_and_omit_the_native_flag() {
    let launcher = crate::launch::test_cache().unwrap();
    let init:Initialization=serde_json::from_value(json!({"account":{"subscriptionType":"Team","apiProvider":"firstParty"},
        "models":[{"value":"simple","resolvedModel":"simple-1","displayName":"Simple","description":"test"}]})).unwrap();
    assert_eq!(
        catalog(&init).unwrap()["data"][0]["supportedReasoningEfforts"][0]["reasoningEffort"],
        "none"
    );
    let dir = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        model: "simple".into(),
        effort: "none".into(),
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let mut command = Command::new("claude");
    let guard = config.configure(&mut command, &launcher).unwrap();
    assert!(!command.get_args().any(|arg| arg == "--effort"));
    let args: Vec<_> = command.get_args().collect();
    let settings = args
        .windows(2)
        .find(|pair| pair[0] == "--settings")
        .unwrap()[1];
    let expected = json!({"sandbox":{
        "enabled":true,"failIfUnavailable":true,"autoAllowBashIfSandboxed":true,
        "allowUnsandboxedCommands":false,"excludedCommands":[],
        "network":{"allowUnixSockets":[guard.directory().join("socket")],"allowAllUnixSockets":false},
        "filesystem":{"denyWrite":["/"]}},
        "permissions":{"deny":["Agent","Task","TaskOutput","TaskStop","CronCreate","CronDelete","CronList"]},
        "autoMemoryEnabled":false});
    assert_eq!(settings.to_str().unwrap(), expected.to_string());
}
