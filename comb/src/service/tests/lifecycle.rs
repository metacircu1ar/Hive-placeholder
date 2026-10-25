use super::*;

#[test]
fn immediate_shutdown_releases_idle_tasks_before_returning() {
    let (dir, server, _) = setup(None);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(server.run(async {})).unwrap();
    // Do not give cancellation another runtime tick before trying to reopen.
    let reopened = Comb::open(dir.path());
    drop(runtime);
    assert!(reopened.is_ok(), "{}", reopened.err().unwrap());
}

#[test]
fn preparation_does_not_publish_a_socket() {
    let dir = crate::test_support::private_directory().unwrap();
    drop(Comb::init(dir.path()).unwrap());
    let path = dir.path().join("comb.sock");
    let prepared = Server::prepare(
        dir.path(),
        Options {
            socket: Some(path.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!path.exists());
    assert!(std::os::unix::net::UnixStream::connect(&path).is_err());
    let server = prepared.bind().unwrap();
    assert!(std::os::unix::net::UnixStream::connect(&path).is_ok());
    drop(server);
    assert!(!path.exists());
}

#[test]
fn no_id_administration_timeout_reports_uncertain_and_work_can_finish_later() {
    let (_dir, server, human) = setup(None);
    let state = server.state.clone();
    let guard = state.writer.lock().unwrap();
    runtime().block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let running = tokio::spawn(server.run_with_deadline(
            async {
                let _ = stopped.await;
            },
            Duration::from_millis(50),
        ));
        let error = call(
            &human,
            &Operation::CredentialCreate {
                grant: Grant::Client {
                    name: "late-client".into(),
                    reviewer: false,
                },
            },
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::Uncertain);
        assert!(error.file_committed);
        assert!(error.request_id.is_none());
        assert!(error.message.contains("inspect"));
        drop(guard);
        embeddings::until(|| state.writes.available_permits() == 8).await;
        let credentials = call(&human, &Operation::CredentialList, None)
            .await
            .unwrap();
        assert_eq!(credentials[0]["grant"]["name"], "late-client");
        stop.send(()).unwrap();
        running.await.unwrap().unwrap();
    });
}
