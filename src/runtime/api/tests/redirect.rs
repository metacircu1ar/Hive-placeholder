use super::*;
use crate::runtime::domain::MessageState;

#[test]
fn failed_batch_rolls_back_all_redirect_evidence_and_allows_the_same_retry_id() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    seed_submitted_redirect_target(&core);
    let db = rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap();
    let counts = || {
        [
            "messages",
            "deliveries",
            "events",
            "requests",
            "redirect_interrupts",
        ]
        .map(|table| {
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
        })
    };
    let before = counts();
    let send = json!({"op":"send","target":"recipient","body":"guidance","mode":"redirect"});
    let mut item = send.clone();
    item["request_id"] = json!("inner");
    let batch = json!({"op":"batch","operations":[item,{"op":"terminate","target":"missing","request_id":"fail"}]});
    assert!(call(&mut core, "human", "batch", batch).is_err());
    assert_eq!(counts(), before);
    assert!(!core.store().agent("recipient").unwrap().redirect_pending);
    assert!(
        core.store()
            .prior(
                "human",
                "inner",
                &RequestEnvelope::new(send.clone()).fingerprint()
            )
            .unwrap()
            .is_none()
    );
    let accepted = call(&mut core, "human", "inner", send.clone()).unwrap();
    assert_eq!(accepted["redirect_target"]["message_id"], "input");
    assert!(core.store().agent("recipient").unwrap().redirect_pending);
    assert_eq!(call(&mut core, "human", "inner", send).unwrap(), accepted);
    assert!(
        core.store()
            .claim_redirect("recipient", "execution", "input", "turn")
            .unwrap()
    );
    assert!(
        !core
            .store()
            .claim_redirect("recipient", "execution", "input", "turn")
            .unwrap()
    );
    let page = core.store().messages("recipient", None, 200).unwrap();
    assert_eq!(page["messages"][0]["mode"], "redirect");
}

fn seed_submitted_redirect_target(core: &Core) {
    let mut target = agent("recipient");
    target.state = AgentState::Running;
    target.execution_id = Some("execution".into());
    target.turn_id = Some("turn".into());
    core.store().save(&target).unwrap();
    core.store()
        .enqueue("input", "human", "recipient", "initial")
        .unwrap();
    let input = core.store().next_message("recipient").unwrap().unwrap();
    core.store()
        .message_state(&input, MessageState::Submitted)
        .unwrap();
    core.store()
        .turn_started(
            &target,
            crate::runtime::api::events::TurnStarted {
                attachments: &[],
                turn_id: "turn",
                message_id: "input",
                sender: "",
                selection: crate::runtime::api::events::TurnSelection {
                    model: &target.model,
                    effort: &target.effort,
                    options_revision: target.options_revision,
                    selection_verification: "requested_unverified",
                },
            },
        )
        .unwrap();
}
