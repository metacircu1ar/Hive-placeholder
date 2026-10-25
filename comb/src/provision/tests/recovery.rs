//! A failure at any journal phase recovers the index and settings together.
use super::*;
use crate::{
    Comb,
    embedding::{Config, Embedder as _, Engine, HttpConfig},
};
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, PathBuf, embedding_http::Server) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    drop(Comb::init(&root).unwrap());
    let root = root.canonicalize().unwrap();
    (temp, root, embedding_http::Server::new())
}
fn http(server: &embedding_http::Server, revision: &str) -> HttpConfig {
    serde_json::from_value(serde_json::json!({"model":"fixture","revision":revision,
        "dimensions":2,"url":server.url}))
    .unwrap()
}
fn active(root: &Path) -> bool {
    Comb::open(root)
        .unwrap()
        .index()
        .unwrap()
        .active_space()
        .unwrap()
        .is_some()
}
#[test]
fn journal_failures_recover_the_index_and_configuration_together() {
    use selection::Phase;
    let (_temp, root, server) = fixture();
    for fault in [
        Phase::BeforeJournal,
        Phase::JournalRenamed,
        Phase::JournalPublished,
        Phase::IndexActivated,
        Phase::ConfigPublished,
    ] {
        select_none(&root).unwrap();
        let error = selection::change_with_hook(
            &root,
            Some(Config::Http(http(&server, "pinned"))),
            None,
            &mut |phase| {
                if phase == fault {
                    Err(invalid("injected publication failure"))
                } else {
                    Ok(())
                }
            },
        )
        .err()
        .unwrap();
        let committed = fault != Phase::BeforeJournal;
        assert_eq!(error.file_committed, committed, "{fault:?}");
        if committed {
            assert_eq!(error.code, crate::ErrorCode::RecoveryRequired, "{fault:?}");
        }
        recover(&root).unwrap();
        let saved = settings(&root).unwrap().unwrap();
        assert_eq!(saved.embeddings.is_some(), committed, "{fault:?}");
        assert_eq!(active(&root), committed, "{fault:?}");
        assert!(!root.join(".comb/model-transition.json").exists());
    }
}
#[test]
fn text_only_deactivates_and_reactivation_preserves_lease_fencing() {
    let (_temp, root, server) = fixture();
    use_http(&root, http(&server, "pinned")).unwrap();
    let engine = Engine::load(Config::Http(http(&server, "pinned"))).unwrap();
    let lease = || {
        Comb::open(&root)
            .unwrap()
            .index()
            .unwrap()
            .embedding_lease(engine.space())
            .unwrap()
    };
    let old = lease();
    assert_eq!(
        select_none(&root).unwrap().backfill,
        "disabled; text search only"
    );
    assert!(!active(&root));
    text_only_health(&root);
    use_http(&root, http(&server, "pinned")).unwrap();
    let new = lease();
    assert_ne!(old.activation, new.activation);
    assert_eq!(old.fingerprint, new.fingerprint);
}
fn text_only_health(root: &Path) {
    use crate::service::{
        self,
        auth::{Connection, ConnectionExt as _},
    };
    let server = service::Server::open(root, service::Options::default()).unwrap();
    let connection = Connection::human(root, service::socket_path(root, None).unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        let health = service::client::call(
            &connection,
            &comb_protocol::service::api::Operation::Health,
            None,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let health: comb_protocol::service::api::Health = serde_json::from_value(health).unwrap();
        assert!(
            health.model.is_none(),
            "doctor must accept intentional text-only setup"
        );
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    });
}
#[test]
fn reselection_repairs_manual_index_changes_even_when_settings_match() {
    let (_temp, root, server) = fixture();
    let selected = Engine::load(Config::Http(http(&server, "pinned"))).unwrap();
    let other = Engine::load(Config::Http(http(&server, "other"))).unwrap();
    select_none(&root).unwrap();
    Comb::open(&root)
        .unwrap()
        .activate_embeddings(selected.space(), true)
        .unwrap();
    assert!(select_none(&root).unwrap().changed);
    assert!(!active(&root));
    use_http(&root, http(&server, "pinned")).unwrap();
    Comb::open(&root)
        .unwrap()
        .activate_embeddings(other.space(), true)
        .unwrap();
    assert!(use_http(&root, http(&server, "pinned")).unwrap().changed);
    assert!(
        Comb::open(&root)
            .unwrap()
            .index()
            .unwrap()
            .embedding_lease(selected.space())
            .is_ok()
    );
}
#[test]
fn setup_custom_socket_is_part_of_the_recoverable_activation_intent() {
    let (_temp, root, server) = fixture();
    let custom = root.join("custom.sock");
    let error = selection::change_with_hook(
        &root,
        Some(Config::Http(http(&server, "pinned"))),
        Some(&custom),
        &mut |phase| {
            if phase == selection::Phase::IndexActivated {
                Err(invalid("injected"))
            } else {
                Ok(())
            }
        },
    )
    .err()
    .unwrap();
    assert!(error.file_committed);
    let options = || crate::setup::Options::default();
    assert!(
        crate::setup::run_offline(&root, None, options())
            .unwrap()
            .changed
    );
    let saved = settings(&root).unwrap().unwrap();
    assert_eq!(saved.socket, custom);
    assert!(matches!(saved.embeddings, Some(Config::Http(config)) if config.revision == "pinned"));
    assert!(
        !crate::setup::run_offline(&root, None, options())
            .unwrap()
            .changed
    );
}
