use super::*;

#[test]
fn catalogs_do_not_reuse_an_old_revision_when_the_same_key_returns() {
    let catalogs = Catalogs::new(
        Runtime::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
    );
    let generation = uuid::Uuid::new_v4();
    let binary = BinaryIdentity::read(&std::env::current_exe().unwrap()).unwrap();
    let first = crate::state::KeyAuthority {
        revision: std::num::NonZeroU64::new(1).unwrap(),
        fingerprint: crate::config::KeyFingerprint::digest(b"a"),
    };
    let mut again = first.clone();
    again.revision = std::num::NonZeroU64::new(3).unwrap();
    catalogs.observe(
        generation,
        Some(first.clone()),
        binary.clone(),
        vec![Model::parse(&serde_json::json!({"model":"old-only"}))],
    );
    assert!(catalogs.cached(generation, Some(&first), &binary).is_some());
    assert!(catalogs.cached(generation, Some(&again), &binary).is_none());
    catalogs.observe(
        generation,
        Some(again.clone()),
        binary.clone(),
        vec![Model::parse(&serde_json::json!({"model":"new-only"}))],
    );
    let models = catalogs.cached(generation, Some(&again), &binary).unwrap();
    assert_eq!(models[0].name(), Some("new-only"));
}
