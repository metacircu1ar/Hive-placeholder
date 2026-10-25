use super::*;
use std::{cell::Cell, sync::mpsc, thread, time::Duration};

fn stages(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(hive_setup_support::names::state_preparation_name)
        })
        .collect()
}

#[test]
fn paused_actual_receipt_writer_prevents_exclusive_cleanup_admission() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let state = State::new(fixture.state.root.clone()).unwrap();
    let binding = fixture.binding.clone();
    let (arrived, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let writer = thread::spawn(move || {
        let first = Cell::new(true);
        with_hook(
            move |path, step| {
                if path.parent().unwrap().file_name().unwrap() == "executions"
                    && step == Step::Written
                    && first.replace(false)
                {
                    arrived.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            },
            || {
                let lease = Arc::new(state.runtime_lease(&binding).unwrap());
                let mut provider = lease.begin_provider(&Uuid::new_v4().to_string()).unwrap();
                provider.finish(true, true).unwrap();
            },
        );
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let candidate = stages(&fixture.binding.directory.join("executions"))
        .pop()
        .unwrap();
    assert_eq!(
        fixture.begin(OperationKind::Login).err().unwrap().code(),
        "profile_busy"
    );
    assert!(candidate.exists());
    release.send(()).unwrap();
    writer.join().unwrap();
    let abandoned = fixture.candidate("executions");
    fixture
        .begin(OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
    assert!(!abandoned.exists());
    assert_eq!(
        fs::read_dir(fixture.binding.directory.join("execution-archive"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn cleanup_itself_holds_index_lock_against_actual_identity_publication() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let candidate = fixture.candidate("");
    let target = candidate.clone();
    let state = State::new(fixture.state.root.clone()).unwrap();
    let binding = fixture.binding.clone();
    let (arrived, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let cleaner = thread::spawn(move || {
        with_hook(
            move |path, step| {
                if path == target && step == Step::BeforeRemove {
                    arrived.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            },
            || state.operation(&binding, OperationKind::Login),
        )
        .unwrap()
        .finish()
        .unwrap();
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let state = State::new(fixture.state.root.clone()).unwrap();
    let binding = fixture.binding.clone();
    let (arrived, admission) = mpsc::channel();
    let (done, completion) = mpsc::channel();
    let writer = thread::spawn(move || {
        let result = with_hook(
            move |path, step| {
                if path.file_name().unwrap() == "index.lock" && step == Step::BeforePublish {
                    arrived.send(()).unwrap();
                }
                Ok(())
            },
            || {
                let _index = state.metadata_lock()?;
                State::initialize_binding(&binding)
            },
        );
        done.send(()).unwrap();
        result.unwrap();
    });
    admission.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(
        completion.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(candidate.exists());
    release.send(()).unwrap();
    completion.recv_timeout(Duration::from_secs(10)).unwrap();
    cleaner.join().unwrap();
    writer.join().unwrap();
    assert!(!candidate.exists());
    let identity: Binding =
        state::read_json(&fixture.binding.directory.join("identity.json")).unwrap();
    assert!(
        identity.ready,
        "the identity publisher completed after cleanup"
    );
}
