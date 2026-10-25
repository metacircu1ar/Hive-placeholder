use super::*;

fn active() -> Agent {
    let mut a = agent("a", None);
    a.state = AgentState::Running;
    a.execution_id = Some("execution".into());
    a.turn_id = Some("turn".into());
    a.steering = crate::runtime::domain::SteeringCapability::Supported;
    a
}

fn states(store: &Store) -> Vec<(String, String)> {
    let messages = store.messages("a", None, 200).unwrap();
    messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["id"].as_str().unwrap().into(),
                m["state"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[test]
fn recovery_expires_unsubmitted_steering_and_never_requeues_uncertain_input() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = temp.path().join("db");
    let s = Store::open(&path).unwrap();
    let a = active();
    s.save(&a).unwrap();
    for (id, status) in [
        ("submitting", MessageState::Submitting),
        ("submitted", MessageState::Submitted),
        ("uncertain", MessageState::Uncertain),
    ] {
        s.enqueue_steering(id, "human", &a, "guidance").unwrap();
        let message = s.next_steering("a", "execution", "turn").unwrap().unwrap();
        s.message_state(&message, status).unwrap();
    }
    s.enqueue_steering("accepted", "human", &a, "guidance")
        .unwrap();
    s.enqueue("next", "human", "a", "ordinary input").unwrap();
    s.defer_submission("a").unwrap();
    s.reject_submission("a").unwrap();
    assert!(states(&s).contains(&("submitting".into(), "submitting".into())));
    assert_eq!(s.next_message("a").unwrap().unwrap().id, "next");
    assert!(
        s.next_steering("a", "other-execution", "turn")
            .unwrap()
            .is_none()
    );
    assert!(
        s.next_steering("a", "execution", "other-turn")
            .unwrap()
            .is_none()
    );
    drop(s);
    let s = Store::open(&path).unwrap();
    s.recover_agents().unwrap();
    assert!(!s.agent("a").unwrap().steering.supported());
    assert_eq!(
        states(&s),
        vec![
            ("next".into(), "accepted".into()),
            ("accepted".into(), "stale_turn".into()),
            ("uncertain".into(), "uncertain".into()),
            ("submitted".into(), "uncertain".into()),
            ("submitting".into(), "uncertain".into()),
        ]
    );
    let counts: (i64, i64) = s
        .connection
        .query_row(
            "SELECT pending_count,mail_count FROM agents WHERE id='a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1));
    assert_eq!(s.next_message("a").unwrap().unwrap().id, "next");
    let messages = s.messages("a", None, 200).unwrap();
    let accepted = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "accepted")
        .unwrap();
    assert_eq!(accepted["delivery_error"], "stale_turn");
}

#[test]
fn completion_updates_only_acknowledged_guidance_for_the_bound_turn() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let a = active();
    s.save(&a).unwrap();
    for (id, status) in [
        ("submitted", MessageState::Submitted),
        ("uncertain", MessageState::Uncertain),
    ] {
        s.enqueue_steering(id, "human", &a, "guidance").unwrap();
        let message = s.next_steering("a", "execution", "turn").unwrap().unwrap();
        s.message_state(&message, status).unwrap();
    }
    s.enqueue_steering("late", "human", &a, "guidance").unwrap();
    for (id, execution, turn) in [
        ("other-execution", "other", "turn"),
        ("other-turn", "execution", "other"),
    ] {
        let mut other = a.clone();
        other.execution_id = Some(execution.into());
        other.turn_id = Some(turn.into());
        s.enqueue_steering(id, "human", &other, "guidance").unwrap();
        let message = s.next_steering("a", execution, turn).unwrap().unwrap();
        s.message_state(&message, MessageState::Submitted).unwrap();
    }
    assert!(s.next_message("a").unwrap().is_none());
    s.finish_steering("a", "execution", "turn", MessageState::Interrupted)
        .unwrap();
    assert_eq!(
        states(&s),
        vec![
            ("other-turn".into(), "submitted".into()),
            ("other-execution".into(), "submitted".into()),
            ("late".into(), "stale_turn".into()),
            ("uncertain".into(), "uncertain".into()),
            ("submitted".into(), "interrupted".into())
        ]
    );
    assert_eq!(
        s.connection
            .query_row("SELECT pending_count FROM agents WHERE id='a'", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
    let messages = s.messages("a", None, 200).unwrap();
    let late = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "late")
        .unwrap();
    assert_eq!(late["delivery_error"], "stale_turn");
}

#[test]
fn steering_finalization_never_scans_ordinary_coordinator_history() {
    let temp = crate::runtime::test_directory();
    let path = temp.path().join("db");
    let s = Store::open(&path).unwrap();
    for sql in [
        super::super::steering::queries::EXPIRE,
        super::super::steering::queries::FINISH,
    ] {
        let plans: Vec<String> = s
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(
                rusqlite::params_from_iter(
                    (0..if sql == super::super::steering::queries::EXPIRE {
                        1
                    } else {
                        4
                    })
                        .map(|_| "a"),
                ),
                |r| r.get(3),
            )
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        let bounded = plans.iter().any(|p| {
            p.starts_with("SEARCH messages ")
                && ((p.contains("INDEX messages_steering ")
                    && p.contains("recipient=? AND state=?"))
                    || (p.contains("INDEX messages_inbox ")
                        && p.contains("recipient=? AND state=? AND mode=?")))
        });
        if sql == super::super::steering::queries::EXPIRE {
            assert!(
                bounded
                    || plans.iter().any(|p| p.starts_with("SEARCH messages ")
                        && p.contains("INDEX messages_pending ")
                        && p.contains("recipient=? AND mode=?")),
                "{plans:?}"
            );
        } else {
            assert!(bounded, "{plans:?}");
            assert!(
                plans
                    .iter()
                    .any(|p| p.contains("SEARCH deliveries") && p.contains("message_id=?")),
                "{plans:?}"
            );
        }
    }
}

#[test]
fn quarantine_wins_over_stale_worker_claims_and_late_acknowledgements() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    let a = active();
    s.save(&a).unwrap();
    for (id, claim) in [("unclaimed", false), ("in-flight", true)] {
        s.enqueue_steering(id, "human", &a, "guidance").unwrap();
        let message = s.next_steering("a", "execution", "turn").unwrap().unwrap();
        if claim {
            assert!(
                s.steering_state(
                    &message,
                    MessageState::Accepted,
                    MessageState::Submitting,
                    None
                )
                .unwrap()
            );
        }
        s.uncertain("a").unwrap();
        assert!(
            !s.steering_state(
                &message,
                MessageState::Accepted,
                MessageState::Submitting,
                None
            )
            .unwrap()
        );
        for outcome in [
            MessageState::Submitted,
            MessageState::Completed,
            MessageState::Failed,
        ] {
            assert!(
                !s.steering_state(&message, MessageState::Submitting, outcome, None)
                    .unwrap()
            );
        }
    }
    assert_eq!(
        states(&s),
        vec![
            ("in-flight".into(), "uncertain".into()),
            ("unclaimed".into(), "stale_turn".into())
        ]
    );
}
