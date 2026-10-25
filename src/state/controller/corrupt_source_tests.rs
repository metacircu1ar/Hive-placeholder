//! Verify isolated replacement retains corrupt source metadata and Claude namespace fences.
use super::*;
use crate::config::{Method, Profile, Provider};
use crate::state::directory;
#[test]
fn corrupt_old_metadata_does_not_block_isolated_codex_and_is_retained() {
    let tmp = tempfile::tempdir().unwrap();
    let old = State::new(tmp.path().join("old")).unwrap();
    directory(&old.root).unwrap();
    write_json(
        &old.root.join("index.json"),
        &serde_json::json!({"corrupt":true}),
    )
    .unwrap();
    let before = fs::read(old.root.join("index.json")).unwrap();
    let fresh = old
        .replacement_root(&tmp.path().join("fresh"), Provider::Codex)
        .unwrap();
    fresh
        .register(
            "codex",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    assert!(
        fresh
            .register(
                "claude",
                &Profile {
                    provider: Provider::Claude,
                    method: Method::Subscription,
                    api_key_env: None,
                    api_key_file: None,
                }
            )
            .is_err()
    );
    assert!(
        old.replacement_root(&tmp.path().join("claude"), Provider::Claude)
            .is_err()
    );
    assert_eq!(fs::read(old.root.join("index.json")).unwrap(), before);
    assert!(!old.root.join("index.lock").exists());
}
