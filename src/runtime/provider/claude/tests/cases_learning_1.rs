//! Learning regressions.
use super::*;

#[test]
fn assistant_summary_precedes_terminal_outcomes_and_resets_between_turns() {
    for outcome in [
        TerminalCase::Completed,
        TerminalCase::Interrupted,
        TerminalCase::Failed,
    ] {
        let reason = outcome.wire();
        let succeeded = matches!(outcome, TerminalCase::Completed);
        let mut f = Fixture::new();
        for text in ["first turn", "second turn"] {
            let turn = f.turn();
            for fragment in [text, "tail"] {
                f.assistant(
                    Some("reused-native-id"),
                    "claude-sonnet-5",
                    json!([{"type":"text","text":fragment}]),
                );
                assert_eq!(f.codec.pop().unwrap()["params"]["item"]["text"], fragment);
                assert!(f.codec.pop().is_none());
            }
            f.codec
                .receive(json!({"type":"result","session_id":f.id,
                "subtype":if succeeded {"success"} else {"error_during_execution"},
                "is_error":!succeeded,"terminal_reason":reason,"user_message_uuid":turn}))
                .unwrap();
            let summary = f.codec.pop().unwrap();
            assert_eq!(summary["params"]["item"]["text"], format!("{text}\ntail"));
            assert_eq!(summary["params"]["turnId"], turn);
            let terminal = f.codec.pop().unwrap();
            assert_eq!(terminal["method"], "turn/completed");
            assert_eq!(terminal["params"]["turn"]["status"], outcome.status());
            assert!(f.codec.pop().is_none());
        }
    }
}
