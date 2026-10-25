use super::*;
use crate::{ErrorCode, disk::publication_hooks as hooks};
use comb_protocol::import::{ImportProgress, ImportState, ImportStatus};
use std::{cell::Cell, fs, io};

#[test]
fn import_partial_errors_and_administration_survive_the_socket_boundary() {
    let (root, server, human) = setup(None);
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("a.md"), "private-a").unwrap();
    fs::write(source.path().join("b.md"), "private-b").unwrap();
    let pending = seed_pending(&server, source.path());
    fs::write(
        root.path().join("b.md"),
        "---\ntitle: Independent\n---\nPrivate edit",
    )
    .unwrap();
    runtime().block_on(async {
        let running = Running::start(server);
        let error = call(
            &human,
            &Operation::Import {
                directory: source.path().into(),
                prefix: None,
            },
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(error.file_committed);
        let progress = error.import_progress.as_ref().unwrap();
        assert_eq!(progress.batch_id, pending.batch_id);
        assert_eq!((progress.completed, progress.unfinished), (1, 1));
        let wire = serde_json::to_string(&error).unwrap();
        assert!(!wire.contains("private"));
        assert!(!wire.contains(root.path().to_str().unwrap()));
        let status: ImportStatus =
            serde_json::from_value(call(&human, &Operation::ImportStatus, None).await.unwrap())
                .unwrap();
        assert_eq!(status.current.unwrap().completed, 1);
        let outsider = issue(
            &human,
            Grant::Client {
                name: "reader".into(),
                reviewer: false,
            },
        )
        .await;
        for op in [
            Operation::ImportStatus,
            Operation::ImportAbandon {
                batch_id: pending.batch_id.clone(),
            },
        ] {
            assert_eq!(
                call(&outsider, &op, None).await.unwrap_err().code,
                ErrorCode::Forbidden
            );
        }
        fs::remove_dir_all(source.path()).unwrap();
        let abandoned: ImportProgress = serde_json::from_value(
            call(
                &human,
                &Operation::ImportAbandon {
                    batch_id: pending.batch_id,
                },
                None,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(abandoned.state, ImportState::Abandoned);
        assert_eq!(abandoned.completed, 1);
        assert_eq!(
            fs::read_to_string(root.path().join("b.md")).unwrap(),
            "---\ntitle: Independent\n---\nPrivate edit"
        );
        running.finish().await;
    });
}

fn seed_pending(server: &Server, source: &std::path::Path) -> ImportProgress {
    let mut writer = server.state.writer.lock().unwrap();
    let count = Cell::new(0);
    let error = hooks::with(
        move |path, step| {
            if path.file_name().unwrap() == "intent.json" && step == atomic_fs::Step::BeforePublish
            {
                count.set(count.get() + 1);
                if count.get() == 2 {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
            }
            Ok(())
        },
        || writer.import(source, None).unwrap_err(),
    );
    *error.import_progress.unwrap()
}
