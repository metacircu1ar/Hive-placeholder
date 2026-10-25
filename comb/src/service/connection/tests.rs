use super::*;
use crate::service::{Operation, tests::setup};

#[test]
fn a_disconnected_committed_request_is_counted_without_replay() {
    let (dir, server, human) = setup(None);
    let state = server.state.clone();
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    state.writer.lock().unwrap().before_index = Some(Box::new(move || {
        entered.send(()).unwrap();
        resumed.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    let request = Request {
        protocol: comb_protocol::compatibility::SERVICE_PROTOCOL,
        credential: human.credential,
        claim: None,
        operation: Operation::Put {
            slug: "concepts/private".parse().unwrap(),
            markdown: "---\ntitle: Private\n---\nnever-log-this-body".into(),
            expected_hash: None,
            request_id: "lost-reply".parse().unwrap(),
            run: None,
        },
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let path = dir.path().join("diagnostics.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.scope(|| {
        runtime.block_on(async {
            let (mut client, accepted) = UnixStream::pair().unwrap();
            let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
            let task = tokio::spawn(serve_connection(
                state.clone(),
                accepted,
                permit,
                Duration::from_secs(5),
            ));
            framing::write(&mut client, &request, framing::REQUEST_BYTES)
                .await
                .unwrap();
            entry.await.unwrap();
            drop(client);
            resume.send(()).unwrap();
            task.await.unwrap();
            assert_eq!(state.observations.count(Loss::PeerGone), 1);
            let replay = state.handle(request).await.unwrap();
            assert_eq!(replay["replayed"], true);
            state.observations.finish();
        });
    });
    assert_eq!(
        guard.finish(Duration::from_secs(1)).state,
        hive_log::Shutdown::Drained
    );
    let log = std::fs::read_to_string(path).unwrap();
    assert!(log.contains("comb.delivery_loss_total"));
    assert!(log.contains("peer_gone"));
    assert!(!log.contains("never-log-this-body"));
    assert!(!log.contains("concepts/private"));
}

#[test]
fn native_output_errors_are_not_all_classified_as_departed_peers() {
    assert!(observations::peer_gone(&Error::from(
        std::io::Error::from_raw_os_error(libc::EPIPE)
    )));
    assert!(!observations::peer_gone(&Error::from(
        std::io::Error::from_raw_os_error(libc::ENOSPC)
    )));
    assert!(!observations::peer_gone(&Error::output_size_limit()));
}

#[test]
fn handler_panics_are_recorded_without_the_panic_payload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    backend.scope(|| {
        runtime.block_on(async {
            let failed = tokio::spawn(async { panic!("private worker panic") });
            observations::joined(failed.await, "connection");
        });
    });
    assert_eq!(
        guard.finish(Duration::from_secs(1)).state,
        hive_log::Shutdown::Drained
    );
    let log = std::fs::read_to_string(path).unwrap();
    assert!(log.contains("comb.task_failed"));
    assert!(!log.contains("private worker panic"));
}
