//! Ignored flags and selection races must not mutate knowledge during rejection.
use super::*;
use crate::{
    Comb,
    embedding::Config,
    service::{Options, Server},
};
use std::{cell::RefCell, fs};

fn fixture() -> (tempfile::TempDir, PathBuf, embedding_http::Server, Config) {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    drop(Comb::init(&root).unwrap());
    let root = root.canonicalize().unwrap();
    let server = embedding_http::Server::new();
    let config = serde_json::from_value(serde_json::json!({"backend":"http","model":"tiny","revision":"pin","dimensions":2,"url":server.url})).unwrap();
    (dir, root, server, config)
}
fn flags() -> Vec<(Options, &'static str)> {
    vec![
        (
            Options {
                replace_embeddings: true,
                ..Options::default()
            },
            "replace_embeddings",
        ),
        (
            Options {
                retry_failed: true,
                ..Options::default()
            },
            "retry_failed",
        ),
        (
            Options {
                vector_cache_mib: Some(512),
                ..Options::default()
            },
            "vector_cache_mib",
        ),
    ]
}
fn snapshot(root: &Path) -> Vec<Option<Vec<u8>>> {
    [
        root.join(".comb/index.sqlite"),
        root.join(".comb/model-transition.json"),
        root.join(".comb/serve.json"),
        root.join("external.md"),
    ]
    .iter()
    .map(crate::test_support::optional_bytes)
    .collect()
}
#[test]
fn absent_model_rejects_each_flag_and_explicit_default_before_socket_work() {
    let (_dir, root, _server, _) = fixture();
    fs::write(root.join("external.md"), "Unparsed external evidence").unwrap();
    let before = snapshot(&root);
    for (options, field) in flags() {
        let error = prepare_server(&root, options).err().unwrap();
        assert_eq!(error.input().unwrap().field(), format!("/options/{field}"));
        assert_eq!(
            error.input().unwrap().code(),
            hive_input::Code::MissingArgument
        );
        assert_eq!(snapshot(&root), before);
    }
    assert!(!root.join(".comb/service.sock").exists());
    assert!(!fs::read_dir(root.join(".comb")).unwrap().any(|p| {
        p.unwrap()
            .file_name()
            .to_string_lossy()
            .contains("comb-socket")
    }));
    for (options, _) in flags() {
        let error = Server::prepare(&root, options).err().unwrap();
        assert!(error.input().is_some());
        assert_eq!(snapshot(&root), before);
    }
}
#[test]
fn saved_and_pending_models_allow_flags_even_with_a_socket_override() {
    for pending in [false, true] {
        let (_dir, root, _server, config) = fixture();
        let socket = root.join(".comb/custom.sock");
        if pending {
            super::serve::prepare_pending(&root, &socket, Some(config));
        } else {
            let Config::Http(config) = config;
            use_http(&root, config).unwrap();
        }
        for (mut options, _) in flags() {
            options.socket = Some(socket.clone());
            drop(prepare_server(&root, options).unwrap());
        }
        drop(prepare_server(&root, Options::default()).unwrap());
    }
}
#[test]
fn pending_text_only_is_rejected_without_finishing_its_activation() {
    let (_dir, root, _server, config) = fixture();
    let Config::Http(config) = config;
    use_http(&root, config).unwrap();
    let socket = settings(&root).unwrap().unwrap().socket;
    super::serve::prepare_pending(&root, &socket, None);
    let before = snapshot(&root);
    for override_socket in [None, Some(root.join("override.sock"))] {
        for (mut options, _) in flags() {
            options.socket = override_socket.clone();
            assert!(
                prepare_server(&root, options)
                    .err()
                    .unwrap()
                    .input()
                    .is_some()
            );
            assert_eq!(snapshot(&root), before);
        }
    }
}
#[test]
fn changing_selection_after_cli_preflight_fails_operationally_before_recovery() {
    let (_dir, root, _server, config) = fixture();
    let Config::Http(config) = config;
    use_http(&root, config).unwrap();
    let plan = plan_server(
        &root,
        Options {
            retry_failed: true,
            ..Options::default()
        },
    )
    .unwrap();
    let socket = settings(&root).unwrap().unwrap().socket;
    super::serve::prepare_pending(&root, &socket, None);
    let before = snapshot(&root);
    let error = plan.prepare().err().unwrap();
    assert_eq!(error.code, crate::ErrorCode::CombBusy);
    assert!(error.input().is_none());
    assert_eq!(snapshot(&root), before);
}
#[test]
fn final_disk_guard_rechecks_a_selection_changed_after_engine_loading() {
    let (_dir, root, _server, config) = fixture();
    let Config::Http(config) = config;
    use_http(&root, config).unwrap();
    let plan = plan_server(
        &root,
        Options {
            vector_cache_mib: Some(1),
            ..Options::default()
        },
    )
    .unwrap();
    let before = RefCell::new(None);
    let error = plan
        .prepare_before_open(|| {
            select_none(&root)?;
            *before.borrow_mut() = Some(snapshot(&root));
            Ok(())
        })
        .err()
        .unwrap();
    assert_eq!(error.code, crate::ErrorCode::CombBusy);
    assert!(error.input().is_none());
    assert_eq!(snapshot(&root), before.into_inner().unwrap());
}
#[test]
fn final_disk_guard_preserves_a_different_selected_model() {
    let (_dir, root, _server, config) = fixture();
    let Config::Http(mut config) = config;
    use_http(&root, config.clone()).unwrap();
    let plan = plan_server(
        &root,
        Options {
            replace_embeddings: true,
            ..Options::default()
        },
    )
    .unwrap();
    config.revision = "other".into();
    let before = RefCell::new(None);
    let error = plan
        .prepare_before_open(|| {
            use_http(&root, config)?;
            *before.borrow_mut() = Some(snapshot(&root));
            Ok(())
        })
        .err()
        .expect("startup must not reactivate the previously loaded model");
    assert_eq!(error.code, crate::ErrorCode::CombBusy);
    assert!(error.input().is_none());
    assert_eq!(snapshot(&root), before.into_inner().unwrap());
}
#[test]
fn corrupt_selection_is_not_reported_as_bad_flags() {
    let (_dir, root, _server, _) = fixture();
    fs::write(root.join(".comb/serve.json"), "PRIVATE corruption").unwrap();
    let error = prepare_server(
        &root,
        Options {
            retry_failed: true,
            ..Options::default()
        },
    )
    .err()
    .unwrap();
    assert!(error.input().is_none());
    assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
}
