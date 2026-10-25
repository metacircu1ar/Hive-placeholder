use super::*;
use crate::{Actor, Comb, index::Index, index::search::QueryVector, read::Snapshot};

fn operations() -> Vec<Operation> {
    include_str!("../../../tests/fixtures/contracts/socket-requests.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Request>(line).unwrap().operation)
        .collect()
}

#[test]
fn every_wire_variant_keeps_its_lane_capability_and_timeout_contract() {
    use Capability::{Capture, Human, Read, Review, Write};
    let expected = [
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Read, Read, false),
        (Class::Admin, Read, false),
        (Class::Write, Write, true),
        (Class::Write, Write, true),
        (Class::Write, Capture, true),
        (Class::Write, Review, true),
        (Class::Write, Write, true),
        (Class::Write, Review, true),
        (Class::Write, Human, true),
        (Class::Admin, Human, true),
        (Class::Admin, Human, true),
        (Class::Admin, Human, false),
        (Class::Admin, Human, true),
        (Class::Admin, Human, true),
        (Class::Admin, Human, false),
        (Class::Admin, Human, true),
    ];
    let operations = operations();
    assert_eq!(operations.len(), expected.len());
    for (operation, (class, capability, mutation)) in operations.iter().zip(expected) {
        assert_eq!(operation.class(), class);
        assert_eq!(operation.required(), capability);
        assert_eq!(operation.is_mutation(), mutation);
        let error = operation.lost_reply();
        assert_eq!(error.file_committed, mutation);
        assert_eq!(error.request_id.as_deref(), operation.request_id());
        let expected = match (mutation, operation.request_id()) {
            (false, _) => Error::reply_timeout(),
            (true, Some(_)) => Error::uncertain_retry(),
            (true, None) => Error::uncertain_inspect(),
        };
        assert_eq!(error.code, expected.code);
        assert_eq!(error.message, expected.message);
    }
}

#[test]
fn collector_projection_uses_the_same_operation_classification() {
    let authority = auth::Authority::new(
        Actor::Collector {
            bridge: "bridge".into(),
        },
        false,
    )
    .unwrap();
    for operation in operations() {
        let allowed = matches!(
            operation,
            Operation::Capabilities | Operation::Health | Operation::Capture { .. }
        );
        assert_eq!(operation.collector_admission(&authority).is_ok(), allowed);
    }
}

#[test]
fn non_text_query_projection_does_not_validate_the_search_early() {
    for mode in [Mode::Text, Mode::Vector, Mode::Hybrid] {
        let operation = Operation::Search {
            request: Search::default(),
            mode,
        };
        let Dispatch::Read(action) = operation.dispatch() else {
            panic!("Search must use a read action")
        };
        assert_eq!(
            action.query(),
            if mode == Mode::Text { None } else { Some("") }
        );
    }
}

#[test]
fn typed_capabilities_execution_keeps_the_authority_reviewer_value() {
    let root = tempfile::tempdir().unwrap();
    let comb = Comb::init(root.path()).unwrap();
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None).unwrap(),
        &comb.publication,
    )
    .unwrap();
    for reviewer in [false, true] {
        let authority = auth::Authority::new(
            Actor::Client {
                id: "client".into(),
                name: "Client".into(),
            },
            reviewer,
        )
        .unwrap();
        let result = ReadAction::Capabilities
            .execute(&snapshot, QueryVector::NotRequested, None, &authority)
            .unwrap();
        assert_eq!(
            result,
            serde_json::json!({"protocols":{"minimum":1,"maximum":1},"reviewer": reviewer})
        );
    }
}
