use super::*;
use crate::{Comb, page::Decision, store::Write};

fn client(reviewer: bool) -> Authority {
    Authority::new(
        Actor::Client {
            id: "client".into(),
            name: "Client".into(),
        },
        reviewer,
    )
    .unwrap()
}

#[test]
fn permissions_keep_service_denials_distinct_from_trusted_store_grants() {
    let authority = client(false);
    assert_eq!(
        authority.permit(Capability::Review).err().unwrap().message,
        Error::configured_reviewer_required().message
    );
    assert_eq!(
        authority.permit(Capability::Human).err().unwrap().message,
        Error::forbidden().message
    );
    for capability in [Capability::Read, Capability::Write, Capability::Capture] {
        assert!(authority.permit(capability).is_ok());
    }
    let request = "review".parse().unwrap();
    assert_eq!(
        Write::new(authority.actor(), &request, None)
            .unwrap()
            .review_for_promotion(false)
            .unwrap_err()
            .message,
        Error::promotion_reviewer_required().message
    );
    assert_eq!(
        Write::new(authority.actor(), &request, None)
            .unwrap()
            .review_for_decision(false)
            .unwrap_err()
            .message,
        Error::decision_reviewer_required().message
    );
    assert!(authority.reviewed_write(&request, None).is_err());
    assert!(client(true).reviewed_write(&request, None).is_ok());
}

#[test]
fn collector_admission_and_final_permission_keep_their_separate_precedence() {
    let authority = Authority::new(
        Actor::Collector {
            bridge: "bridge".into(),
        },
        false,
    )
    .unwrap();
    assert!(authority.permit(Capability::Capture).is_ok());
    for capability in [
        Capability::Read,
        Capability::Write,
        Capability::Review,
        Capability::Human,
    ] {
        assert_eq!(
            authority
                .collector_admission(capability)
                .unwrap_err()
                .message,
            Error::collector_capture_only().message
        );
    }
    assert_eq!(
        authority.permit(Capability::Human).err().unwrap().message,
        Error::forbidden().message
    );
    assert_eq!(
        authority.permit(Capability::Review).err().unwrap().message,
        Error::configured_reviewer_required().message
    );
    assert_eq!(
        authority.permit(Capability::Write).err().unwrap().message,
        Error::collector_capture_only().message
    );
}

#[test]
fn service_review_proof_persists_its_own_actor_principal_and_request() {
    let root = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    let slug = "review/proposal".parse().unwrap();
    let seed = Write::new(&Actor::Human, &"seed".parse().unwrap(), None).unwrap();
    let created = comb
        .put_markdown(&slug, "---\ntitle: Proposal\n---\nText", None, &seed)
        .unwrap();
    let authority = client(true);
    let request = "review".parse().unwrap();
    let proof = authority
        .reviewed_write(&request, Some("review-run"))
        .unwrap();
    let promoted = comb.promote(&slug, &created.hash, &proof).unwrap();
    let page = comb.get(&slug).unwrap().page;
    assert_eq!(
        &page.metadata.owned().promotion().unwrap().by,
        authority.actor()
    );
    let receipt = comb
        .disk
        .receipt(authority.principal(), &request)
        .unwrap()
        .unwrap();
    assert_eq!(&receipt.principal, authority.principal());
    assert_eq!(receipt.request_id, promoted.request_id);
    assert!(comb.promote(&slug, &created.hash, &proof).unwrap().replayed);
}

#[test]
fn human_grants_remain_valid_when_the_explicit_grant_is_false() {
    let root = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    let slug = "review/proposal".parse().unwrap();
    let seed = Write::new(&Actor::Human, &"seed".parse().unwrap(), None).unwrap();
    let created = comb
        .put_markdown(&slug, "---\ntitle: Proposal\n---\nText", None, &seed)
        .unwrap();
    let promote = Write::new(&Actor::Human, &"promote".parse().unwrap(), None)
        .unwrap()
        .review_for_promotion(false)
        .unwrap();
    let promoted = comb.promote(&slug, &created.hash, &promote).unwrap();
    let decide = Write::new(&Actor::Human, &"decide".parse().unwrap(), None)
        .unwrap()
        .review_for_decision(false)
        .unwrap();
    let decided = comb
        .decide(&slug, &promoted.hash, Decision::Accept, "Reviewed", &decide)
        .unwrap();
    assert_eq!(decided.request_id.as_str(), "decide");
}
