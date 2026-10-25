//! Recovery regressions.
use super::*;

#[test]
fn conversation_receipts_reject_duplicate_and_symlinked_directories() {
    for defect in ["duplicate", "symlink"] {
        let (tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let lease = Arc::new(state.runtime_lease(&binding).unwrap());
        let id = Uuid::new_v4().to_string();
        let mut evidence = lease.begin_provider(&id).unwrap();
        evidence.started(std::process::id()).unwrap();
        evidence.finish(true, true).unwrap();
        drop(evidence);
        drop(lease);
        assert!(state.check_conversation(&binding, Some(&id)).is_ok());
        let dir = binding.directory.clone();
        let archive = dir.join("execution-archive");
        if defect == "duplicate" {
            directory(&dir.join("executions")).unwrap();
            fs::copy(
                archive.join(format!("{id}.json")),
                dir.join(format!("executions/{id}.json")),
            )
            .unwrap();
        } else {
            let moved = tmp.path().join("archive");
            fs::rename(&archive, &moved).unwrap();
            std::os::unix::fs::symlink(moved, &archive).unwrap();
        }
        assert!(state.check_conversation(&binding, Some(&id)).is_err());
    }
}

#[test]
fn pruning_refuses_dangling_symlinks_and_empty_databases_but_accepts_current_or_absent() {
    for defect in ["runtime", "receipt", "empty", "current", "absent"] {
        let (tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let sentinel = binding.directory.join("provider/sentinel");
        fs::write(&sentinel, "retained").unwrap();
        let repair = state.begin_repair("main", &binding.profile).unwrap();
        state.finish_repair(&repair, true, None).unwrap();
        directory(&state.root.join("runtime")).unwrap();
        let path = if defect == "receipt" {
            binding.directory.join("prune-receipt.json")
        } else {
            state.root.join("runtime/runtime.sqlite")
        };
        if matches!(defect, "empty" | "current") {
            crate::state::private_file(&path, true).unwrap();
            if defect == "current" {
                let connection = rusqlite::Connection::open(&path).unwrap();
                crate::runtime::initialize_schema(&connection).unwrap();
            }
        } else if defect != "absent" {
            std::os::unix::fs::symlink(tmp.path().join("absent"), &path).unwrap();
        }
        if matches!(defect, "current" | "absent") {
            state.prune("main", binding.id).unwrap();
            assert!(!sentinel.exists());
        } else {
            let error = state.prune("main", binding.id).unwrap_err();
            if defect == "empty" {
                assert_eq!(error.code(), "unsupported_schema");
                assert!(fs::read(&path).unwrap().is_empty());
            }
            assert_eq!(fs::read_to_string(sentinel).unwrap(), "retained");
        }
    }
}
