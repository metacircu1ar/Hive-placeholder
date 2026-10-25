//! A rejected serve must not finish a pending activation as a side effect.
use super::*;
use crate::{
    Comb,
    embedding::Config,
    service::{Options, Server},
};
use std::{fs, os::unix::fs::PermissionsExt as _};

#[test]
fn pending_socket_and_explicit_engine_are_checked_before_activation_recovery() {
    let dir = crate::test_support::private_directory().unwrap();
    let root = dir.path().join("root");
    drop(Comb::init(&root).unwrap());
    let root = root.canonicalize().unwrap();
    let socket = dir.path().join("pending.sock");
    let server = embedding_http::Server::new();
    let config: Config = serde_json::from_value(serde_json::json!({"backend":"http","model":"tiny","revision":"pin","dimensions":2,"url":server.url})).unwrap();
    prepare_pending(&root, &socket, Some(config.clone()));
    assert!(pending_settings(&root).unwrap().is_some());
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let index = root.join(".comb/index.sqlite");
    let before = fs::read(&index).unwrap();
    let journal = fs::read(root.join(".comb/model-transition.json")).unwrap();
    let mut invalid_config = config;
    let Config::Http(http) = &mut invalid_config;
    http.url = "invalid".into();
    for options in [
        Options::default(),
        Options {
            embeddings: Some(invalid_config),
            ..Options::default()
        },
        Options {
            vector_cache_mib: Some(0),
            ..Options::default()
        },
    ] {
        assert!(prepare_server(&root, options).is_err());
        assert_eq!(fs::read(&index).unwrap(), before);
        assert_eq!(
            fs::read(root.join(".comb/model-transition.json")).unwrap(),
            journal
        );
        assert!(!root.join(".comb/serve.json").exists());
    }
    drop(listener);
    fs::remove_file(&socket).unwrap();
    let prepared = prepare_server(&root, Options::default()).unwrap();
    drop(prepared);
    assert!(!root.join(".comb/model-transition.json").exists());
    assert!(
        Comb::open(&root)
            .unwrap()
            .index()
            .unwrap()
            .active_space()
            .unwrap()
            .is_some()
    );
    // Ordinary library preparation still works with the recovered selection.
    drop(
        Server::prepare(
            &root,
            Options {
                socket: Some(socket),
                ..Options::default()
            },
        )
        .unwrap(),
    );
}

pub(super) fn prepare_pending(root: &Path, socket: &Path, config: Option<Config>) {
    let error = selection::change_with_hook(root, config, Some(socket), &mut |phase| {
        if phase == selection::Phase::JournalPublished {
            Err(invalid("test stop after journal"))
        } else {
            Ok(())
        }
    })
    .err()
    .unwrap();
    assert!(error.file_committed, "{error:?}: {:?}", error.diagnostic());
}

#[test]
fn socket_privacy_admission_precedes_explicit_engine_loading_and_pending_recovery() {
    let dir = crate::test_support::private_directory().unwrap();
    let root = dir.path().join("root");
    let parent = dir.path().join("public");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = parent.join("s.sock");
    drop(Comb::init(&root).unwrap());
    let root = root.canonicalize().unwrap();
    prepare_pending(&root, &socket, None);
    let paths = [
        root.join(".comb/index.sqlite"),
        root.join(".comb/model-transition.json"),
    ];
    let before = paths
        .iter()
        .map(|p| fs::read(p).unwrap())
        .collect::<Vec<_>>();
    let mut config: Config = serde_json::from_value(serde_json::json!({"backend":"http",
        "model":"tiny", "revision":"pin", "dimensions":2,
        "url":"http://127.0.0.1:9/embeddings"}))
    .unwrap();
    let Config::Http(http) = &mut config;
    http.url = "invalid engine URL".into();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    for options in [
        Options::default(),
        Options {
            embeddings: Some(config),
            ..Options::default()
        },
    ] {
        let error = prepare_server(&root, options).err().unwrap();
        assert_eq!(error.message, crate::Error::unsafe_socket_parent().message);
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        for (path, bytes) in paths.iter().zip(&before) {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn replaced_root_is_rejected_before_pending_recovery_touches_index_or_journal() {
    let dir = crate::test_support::private_directory().unwrap();
    let root = dir.path().join("root");
    let moved = dir.path().join("original");
    let socket = dir.path().join("shared.sock");
    drop(Comb::init(&root).unwrap());
    let lease = Server::preflight(
        &root,
        &Options {
            socket: Some(socket.clone()),
            ..Options::default()
        },
    )
    .unwrap();
    fs::rename(&root, &moved).unwrap();
    drop(Comb::init(&root).unwrap());
    let root = root.canonicalize().unwrap();
    let server = embedding_http::Server::new();
    let config = serde_json::from_value(serde_json::json!({"backend":"http","model":"tiny","revision":"pin","dimensions":2,"url":server.url})).unwrap();
    prepare_pending(&root, &socket, Some(config));
    let paths = [
        root.join(".comb/index.sqlite"),
        root.join(".comb/model-transition.json"),
    ];
    let before: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
    assert!(selection::recover_checked(&root, &|disk| lease.check_disk(disk)).is_err());
    for (path, expected) in paths.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
    assert!(!root.join(".comb/serve.json").exists());
}
