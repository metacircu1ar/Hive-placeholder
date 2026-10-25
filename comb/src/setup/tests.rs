use super::*;
use crate::service::{self, Operation, auth::Connection};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt as _};
#[path = "tests/intent.rs"]
mod intent_recovery;
mod socket;

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
fn args(bridge: &Path) -> Options {
    Options {
        for_hive: true,
        bridge_file: Some(bridge.into()),
        reviewer_role: vec!["reviewer".into()],
        ..Options::default()
    }
}

#[test]
fn fresh_setup_reuses_custom_socket_and_bridge_without_changes() {
    let temp = crate::test_support::private_directory().unwrap();
    let root = temp.path().join("root");
    let credential = temp.path().join("config/bridge.json");
    let socket = temp.path().join("custom.sock");
    assert!(
        run_offline(&root, Some(&socket), args(&credential))
            .unwrap()
            .changed
    );
    assert_eq!(
        fs::metadata(&credential).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let before = snapshot(temp.path());
    assert!(!run_offline(&root, None, args(&credential)).unwrap().changed);
    assert_eq!(snapshot(temp.path()), before);
    assert!(
        run_offline(
            &root,
            Some(&temp.path().join("wrong.sock")),
            args(&credential)
        )
        .is_err()
    );
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn dry_run_and_invalid_bridge_never_recover_or_create_the_index() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let credential = temp.path().join("bridge.json");
    let mut options = args(&credential);
    options.dry_run = true;
    run_offline(&root, None, options).unwrap();
    assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
    run_offline(&root, None, args(&credential)).unwrap();
    fs::remove_file(&credential).unwrap();
    fs::remove_file(root.join(".comb/index.sqlite")).unwrap();
    let saved = provision::settings(&root).unwrap().unwrap();
    files::write_json(&root.join(".comb/model-transition.json"), &saved).unwrap();
    let before = snapshot(temp.path());
    let mut options = args(&credential);
    options.dry_run = true;
    run_offline(&root, None, options).unwrap();
    assert_eq!(snapshot(temp.path()), before);
    fs::write(&credential, b"invalid").unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let before = snapshot(temp.path());
    assert!(run_offline(&root, None, args(&credential)).is_err());
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn existing_unmanaged_root_requires_explicit_selection_and_busy_root_fails() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let busy = Comb::init(&root).unwrap();
    let before = snapshot(temp.path());
    let error = run_offline(&root, None, Options::default()).err().unwrap();
    assert!(error.message.contains("comb model use none"), "{error:?}");
    assert!(provision::select_none(&root).is_err());
    assert_eq!(snapshot(temp.path()), before);
    drop(busy);
    provision::select_none(&root).unwrap();
    assert!(
        !run_offline(&root, None, Options::default())
            .unwrap()
            .changed
    );
}

#[test]
fn foreign_revoked_wrong_secret_and_role_credentials_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let path = temp.path().join("bridge.json");
    run_offline(&root, None, args(&path)).unwrap();
    let original = fs::read(&path).unwrap();
    let mut options = args(&path);
    options.reviewer_role.push("another".into());
    assert!(run_offline(&root, None, options).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut connection = Connection::read(&path).unwrap();
    connection.credential.secret = "0".repeat(64);
    fs::write(&path, serde_json::to_vec(&connection).unwrap()).unwrap();
    assert!(run_offline(&root, None, args(&path)).is_err());
    fs::write(&path, &original).unwrap();
    let other = temp.path().join("other");
    assert!(run_offline(&other, None, args(&path)).is_err());
    assert!(!other.exists());
    let connection = Connection::read(&path).unwrap();
    service::offline(
        &mut Comb::open(&root).unwrap(),
        &Operation::CredentialRevoke {
            id: connection.credential.id,
        },
        None,
    )
    .unwrap();
    let before = snapshot(temp.path());
    assert!(run_offline(&root, None, args(&path)).is_err());
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn symlink_credential_is_refused_without_touching_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let credential = temp.path().join("actual.json");
    run_offline(&root, None, args(&credential)).unwrap();
    let link = temp.path().join("link.json");
    std::os::unix::fs::symlink(&credential, &link).unwrap();
    let before = fs::read(&credential).unwrap();
    assert!(run_offline(&root, None, args(&link)).is_err());
    assert_eq!(fs::read(&credential).unwrap(), before);
}

#[test]
fn socket_files_and_bridge_collisions_fail_before_initialization() {
    let temp = crate::test_support::private_directory().unwrap();
    let root = temp.path().join("root");
    let socket = temp.path().join("socket");
    let credential = temp.path().join("bridge");
    fs::write(&socket, b"do not replace").unwrap();
    let before = snapshot(temp.path());
    assert!(run_offline(&root, Some(&socket), args(&credential)).is_err());
    assert_eq!(snapshot(temp.path()), before);
    fs::remove_file(&socket).unwrap();
    assert!(run_offline(&root, Some(&socket), args(&socket)).is_err());
    assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
}
#[test]
fn fresh_setup_choice_survives_interruption_before_activation_journal() {
    let temp = crate::test_support::private_directory().unwrap();
    let root = temp.path().join("root");
    let socket = temp.path().join("custom.sock");
    let plan = Plan::inspect(&root, Some(&socket), &Options::default()).unwrap();
    intent::publish(&plan).unwrap();
    drop(Comb::init(&root).unwrap());
    assert!(
        run_offline(&root, None, Options::default())
            .unwrap()
            .changed
    );
    assert_eq!(
        provision::settings(&root).unwrap().unwrap().socket,
        match socket.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => plan.socket.clone(),
            Err(error) => panic!("fixture socket identity failed: {error}"),
        }
    );
    assert!(!root.join(".comb-setup.json").exists());
    assert!(
        !run_offline(&root, None, Options::default())
            .unwrap()
            .changed
    );
}

#[test]
fn missing_leaves_through_symlinked_parents_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = temp.path().join("linked");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let root = link.join("root");
    let bridge = link.join("config/bridge.json");
    let socket = link.join("sockets/service.sock");
    let result = run_offline(&root, Some(&socket), args(&bridge)).unwrap();
    assert!(result.changed);
    assert_eq!(result.plan.root, real.canonicalize().unwrap().join("root"));
    assert_eq!(
        result.plan.socket,
        real.canonicalize().unwrap().join("sockets/service.sock")
    );
    let leaf = temp.path().join("leaf");
    std::os::unix::fs::symlink(&root, &leaf).unwrap();
    assert!(run_offline(&leaf, Some(&socket), args(&bridge)).is_err());
}

#[test]
fn stale_owned_socket_is_reclaimable_but_listening_socket_is_not() {
    use std::os::unix::net::UnixListener;
    let temp = crate::test_support::private_directory().unwrap();
    let root = temp.path().join("root");
    let socket = temp.path().join("service.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(run_offline(&root, Some(&socket), Options::default()).is_err());
    assert!(!root.exists());
    drop(listener);
    assert!(
        run_offline(&root, Some(&socket), Options::default())
            .unwrap()
            .changed
    );
    assert!(socket.exists()); // Only the server reclaims it, not setup preflight.
}

#[test]
fn shared_sticky_socket_parent_is_rejected() {
    let parent = Path::new("/tmp").canonicalize().unwrap();
    assert!(validate::socket_parent(&parent).is_err());
}

#[test]
fn unchanged_http_setup_can_add_bridge_without_credentials_or_requests() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    run_offline(&root, None, Options::default()).unwrap();
    // Model activation itself requires a working endpoint. Seed the exact already
    // activated state here, then make the endpoint a nonblocking request detector.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config: crate::embedding::HttpConfig = serde_json::from_value(serde_json::json!({
        "model":"test", "revision":"fixed", "url":format!("http://{}/v1/embeddings", listener.local_addr().unwrap()),
        "dimensions": 3, "api_key_env":"HIVE_TEST_SETUP_ABSENT_SECRET_59993"
    })).unwrap();
    assert!(std::env::var_os("HIVE_TEST_SETUP_ABSENT_SECRET_59993").is_none());
    let space = crate::embedding::Engine::http_identity(config.clone()).unwrap();
    Comb::open(&root)
        .unwrap()
        .activate_embeddings(&space, true)
        .unwrap();
    let mut settings = provision::settings(&root).unwrap().unwrap();
    settings.embeddings = Some(crate::embedding::Config::Http(config));
    files::write_json(&root.join(".comb/serve.json"), &settings).unwrap();
    let before = fs::read(root.join(".comb/serve.json")).unwrap();
    let bridge = temp.path().join("bridge.json");
    assert!(run_offline(&root, None, args(&bridge)).unwrap().changed);
    assert_eq!(fs::read(root.join(".comb/serve.json")).unwrap(), before);
    assert!(bridge.is_file());
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn explicit_http_preflight_is_readonly_and_failed_probe_leaves_no_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap();
    drop(listener);
    let file = temp.path().join("http.json");
    fs::write(&file,serde_json::to_vec(&serde_json::json!({"backend":"http","model":"test","revision":"fixed","url":format!("http://{port}/v1/embeddings"),"dimensions":3,"timeout_seconds":1})).unwrap()).unwrap();
    let options = Options {
        embedding_config: Some(file),
        no_services: true,
        dry_run: true,
        ..Options::default()
    };
    let result = run_offline(&root, None, options.clone()).unwrap();
    assert!(result.plan.http.is_some());
    assert!(!root.exists());
    let mut apply = options.clone();
    apply.dry_run = false;
    assert!(run_offline(&root, None, apply).is_err());
    assert!(!root.exists());
    // An interrupted accepted fresh choice survives initialization and omitted flags.
    intent::publish(&result.plan).unwrap();
    drop(Comb::init(&root).unwrap());
    let retry = Plan::inspect(&root, None, &Options::default()).unwrap();
    assert_eq!(retry.http, result.plan.http);
    let other = temp.path().join("other.json");
    fs::write(&other,serde_json::to_vec(&serde_json::json!({"backend":"http","model":"other","revision":"fixed","url":format!("http://{port}/v1/embeddings"),"dimensions":3})).unwrap()).unwrap();
    assert!(
        Plan::inspect(
            &root,
            None,
            &Options {
                embedding_config: Some(other),
                ..Options::default()
            }
        )
        .is_err()
    );
}

#[test]
fn raw_role_validation_precedes_root_and_model_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("absent");
    for roles in [
        vec!["repeat".into(), "repeat".into()],
        (0..65).map(|i| format!("role{i}")).collect(),
    ] {
        let options = Options {
            reviewer_role: roles,
            for_hive: true,
            ..Options::default()
        };
        let error = Plan::inspect(&root, None, &options).err().unwrap();
        assert_eq!(error.input().unwrap().field(), "/options/reviewer_role");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn reserved_outputs_fail_before_setup_or_credential_issuance() {
    for (socket_reserved, name) in [false, true].into_iter().flat_map(|socket| {
        [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"].map(|name| (socket, name))
    }) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let reserved = temp.path().join(name);
        let ordinary = temp.path().join("ordinary");
        let (socket, bridge) = if socket_reserved {
            (&reserved, &ordinary)
        } else {
            (&ordinary, &reserved)
        };
        assert!(run_offline(&root, Some(socket), args(bridge)).is_err());
        assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let bridge = temp.path().join("bridge.json");
    run_offline(&root, None, args(&bridge)).unwrap();
    let before = snapshot(temp.path());
    let reserved = temp.path().join(".hive-tmp-state-0123456789abcdef");
    assert!(run_offline(&root, None, args(&reserved)).is_err());
    assert_eq!(snapshot(temp.path()), before);
    assert!(crate::service::auth::CredentialOutput::prepare(&reserved).is_err());
    assert_eq!(snapshot(temp.path()), before);
}

#[test]
fn group_writable_existing_root_and_bridge_parent_work_through_setup() {
    let temp = crate::test_support::private_directory().unwrap();
    let root = temp.path().join("root");
    let config = temp.path().join("config");
    for directory in [&root, &config] {
        fs::create_dir(directory).unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o775)).unwrap();
    }
    let credential = config.join("bridge.json");
    let socket = temp.path().join("s.sock");
    let first = run_offline(&root, Some(&socket), args(&credential));
    if rustix::process::geteuid().is_root() {
        assert!(first.is_err());
        return;
    }
    assert!(first.unwrap().changed);
    assert!(
        !run_offline(&root, Some(&socket), args(&credential))
            .unwrap()
            .changed
    );
    let connection = crate::service::auth::Connection::read(&credential).unwrap();
    assert_eq!(
        connection.socket,
        socket
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("s.sock")
    );
    for directory in [&root, &config] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o775
        );
    }
    assert_eq!(
        fs::metadata(root.join(".comb"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&credential).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
