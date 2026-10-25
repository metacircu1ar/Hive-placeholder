use super::*;
use comb::service::auth::ConnectionExt;
use std::{path::Path, sync::mpsc, thread};

fn context(root: &Path) -> Context {
    Context {
        comb: root.to_owned(),
        socket: Some(root.join("test.sock")),
        run: None,
    }
}
fn create(path: &Path) -> Plan {
    plan(CredentialCommand::Create {
        name: "test-client".into(),
        out: path.to_owned(),
        bridge: false,
        reviewer: false,
        reviewer_role: Vec::new(),
    })
    .unwrap()
}
fn list(context: &Context) -> Value {
    Plan::print(Operation::CredentialList)
        .execute(context)
        .unwrap()
}

#[test]
fn offline_save_collision_revokes_only_the_issued_credential() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    drop(Comb::init(&root).unwrap());
    let context = context(&root);
    let prior = dir.path().join("prior.json");
    create(&prior).execute(&context).unwrap();
    let retained = list(&context);
    let path = dir.path().join("client.json");
    let mut command = create(&path);
    let Operation::CredentialCreate {
        grant: Grant::Client { name, .. },
    } = &mut command.operation
    else {
        panic!("create");
    };
    *name = "second-client".into();
    assert!(!path.exists());
    std::fs::write(&path, "unrelated").unwrap();
    let error = command.execute(&context).unwrap_err();
    assert!(error.message.contains("has been revoked"), "{error}");
    assert_eq!(list(&context), retained);
    assert_eq!(std::fs::read(&path).unwrap(), b"unrelated");
    assert_no_reservation(dir.path());
}

#[test]
fn changed_output_parent_permissions_refuse_publication_and_revoke() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    drop(Comb::init(&root).unwrap());
    let context = context(&root);
    let output = dir.path().join("output");
    std::fs::create_dir(&output).unwrap();
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = output.join("client.json");
    let command = create(&path);
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o777)).unwrap();
    let error = command.execute(&context).unwrap_err();
    assert_eq!(error.code, comb::ErrorCode::UnsafePath);
    assert!(!error.file_committed);
    assert!(error.message.contains("has been revoked"), "{error}");
    assert_eq!(list(&context), serde_json::json!([]));
    assert!(!path.exists());
    assert_no_reservation(&output);
}

#[test]
fn successful_save_preserves_connection_and_failed_issuance_cancels_reservation() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    let context = context(&root);
    let path = dir.path().join("client.json");
    let command = create(&path);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(command.execute(&context).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    drop(Comb::init(&root).unwrap());
    let result = create(&path).execute(&context).unwrap();
    let saved = Connection::read(&path).unwrap();
    assert_eq!(
        saved.socket,
        service::socket_path(&root, context.socket.as_deref()).unwrap()
    );
    assert_eq!(
        result,
        serde_json::json!({"credential_id":saved.credential.id,"file":path})
    );
    assert!(!result.to_string().contains(&saved.credential.secret));
}

#[test]
fn cleanup_refuses_a_replaced_root_and_wrong_revocation_acknowledgment() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    drop(Comb::init(&root).unwrap());
    let context = context(&root);
    let human = Connection::human(&root, context.socket.clone().unwrap()).unwrap();
    std::fs::rename(&root, dir.path().join("original")).unwrap();
    drop(Comb::init(&root).unwrap());
    let before = list(&context);
    let id = CredentialId::generate();
    let error = revoke(&context, &human, &crate::cli::runtime().unwrap(), &id).unwrap_err();
    assert_eq!(error.code, comb::ErrorCode::Forbidden);
    assert_eq!(list(&context), before);
    for reply in [
        serde_json::json!({"revoked":CredentialId::generate()}),
        serde_json::json!([id]),
        serde_json::json!(null),
        serde_json::json!(id),
    ] {
        assert!(check_revoked(reply, &id).is_err());
    }
    assert!(check_revoked(serde_json::json!({"revoked":id}), &id).is_ok());
}

#[test]
fn publication_outcomes_keep_primary_causes_and_do_not_revoke_visible_files() {
    let id = CredentialId::generate();
    let source = || Error::from(std::io::Error::from_raw_os_error(libc::ENOSPC));
    let error = publication_result(Err(source()), &id, || {
        Err(Error::from(std::io::Error::from_raw_os_error(libc::EIO)))
    })
    .unwrap_err();
    assert_eq!(error.code, comb::ErrorCode::IoError);
    assert!(error.message.contains(id.as_str()));
    assert!(error.message.contains("unconfirmed"));
    let diagnostic = error.diagnostic().unwrap();
    assert!(
        diagnostic.contains("No space") && diagnostic.contains("Input/output"),
        "{diagnostic}"
    );
    let error = publication_result(Err(source().committed()), &id, || panic!("must not revoke"))
        .unwrap_err();
    assert!(error.file_committed);
    assert!(error.message.contains("remains active"));
}

struct Running {
    signal: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Running {
    fn start(root: &Path) -> Self {
        let server = service::Server::open(
            root,
            service::Options {
                socket: Some(root.join("test.sock")),
                ..service::Options::default()
            },
        )
        .unwrap();
        let (signal, stop) = tokio::sync::oneshot::channel();
        let (ready, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = crate::cli::runtime().unwrap();
            runtime.block_on(async {
                ready.send(()).unwrap();
                server
                    .run(async {
                        let _ = stop.await;
                    })
                    .await
                    .unwrap();
            });
        });
        receiver.recv().unwrap();
        Self {
            signal: Some(signal),
            thread: Some(thread),
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(signal) = self.signal.take() {
            let _ = signal.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
fn live_local_service_revokes_a_known_save_failure_without_offline_replay() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    drop(Comb::init(&root).unwrap());
    let context = context(&root);
    let _service = Running::start(&root);
    let path = dir.path().join("client.json");
    let command = create(&path);
    std::fs::write(&path, "unrelated").unwrap();
    let error = command.execute(&context).unwrap_err();
    assert!(error.message.contains("has been revoked"), "{error}");
    assert_eq!(list(&context), serde_json::json!([]));
    assert_eq!(std::fs::read(path).unwrap(), b"unrelated");
}

#[test]
fn lost_or_malformed_issuance_reply_cancels_output_without_replay() {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };
    for malformed in [false, true] {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let root = dir.path().join("knowledge");
        drop(Comb::init(&root).unwrap());
        let context = context(&root);
        let socket = context.socket.as_ref().unwrap();
        let listener = UnixListener::bind(socket).unwrap();
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let peer = listener.try_clone().unwrap();
        let worker = thread::spawn(move || {
            let (stream, _) = peer.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut input = BufReader::new(stream);
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            assert!(line.contains("credential_create"));
            if malformed {
                input
                    .get_mut()
                    .write_all(b"{\"status\":\"ok\",\"result\":null}\n")
                    .unwrap();
            }
        });
        let output = dir.path().join("credential.json");
        assert!(create(&output).execute(&context).is_err());
        worker.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(listener);
        assert!(!output.exists());
        assert_no_reservation(dir.path());
        assert_eq!(list(&context), serde_json::json!([]));
    }
}

fn assert_no_reservation(path: &Path) {
    assert!(std::fs::read_dir(path).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".comb-tmp-")
    }));
}
