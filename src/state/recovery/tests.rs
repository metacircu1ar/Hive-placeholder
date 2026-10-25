//! Verify strict ownership evidence and credential recovery boundaries.

use super::*;
use crate::{
    config::{Method, Profile, Provider},
    state::{GenerationState, directory, read_json, write_json},
};
use std::{fs, io::Write, os::fd::AsRawFd, sync::Arc};

// Spawn-free tests hold fork_guard after fixture setup so unrelated forks cannot
// transiently retain credential OFDs across immediate ownership assertions.
fn fixture() -> (tempfile::TempDir, State, Binding) {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let _fork = crate::os::fork_guard().unwrap();
    let binding = state
        .register(
            "main",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    (tmp, state, binding)
}

fn snapshot_files(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    let mut snapshot = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            snapshot.insert(path.clone(), None);
            pending.extend(
                fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        } else {
            snapshot.insert(path.clone(), Some(fs::read(path).unwrap()));
        }
    }
    snapshot
}

#[test]
#[ignore = "subprocess fixture"]
fn launch_crash_fixture() {
    let root = std::env::var("HIVE_RECOVERY_TEST_ROOT").expect("subprocess fixture input");
    let phase = std::env::var("HIVE_RECOVERY_TEST_PHASE").unwrap();
    let state = State::new(root.into()).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.lookup("main", &profile).unwrap().unwrap();
    if phase == "writer_pending" {
        let mut operation = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let _receipt = operation.prepare_child(true, true).unwrap();
        let records = ledger::records_in(&binding.directory, "writers").unwrap();
        let record = &records[0];
        // The real crash window between durable mutation intent and started().
        state
            .mark_credential_attempt(binding.id, record.run, record.id)
            .unwrap();
        std::process::exit(79);
    }
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let id = std::env::var("HIVE_RECOVERY_TEST_PROVIDER").unwrap();
    let mut evidence = lease.begin_provider(&id).unwrap();
    if phase == "intent" {
        std::process::exit(79);
    }
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args([
            "-c",
            "echo $$ > native.pid.tmp; mv native.pid.tmp native.pid; i=0; while [ ! -e native.stop ] && [ $i -lt 200 ]; do /bin/sleep 0.05; i=$((i+1)); done",
        ])
        .current_dir(&state.root);
    let launcher = crate::launch::test_cache().unwrap();
    let _child = crate::launch::spawn(
        &launcher,
        &command,
        true,
        &[lease.fd().as_raw_fd()],
        |pid| {
            write_json(
                &state.root.join("shim.json"),
                &ProcessIdentity::read(pid)?.unwrap(),
            )?;
            evidence.bind_conversation("agent", "execution", "native")?;
            if phase == "bound" {
                std::process::exit(79);
            }
            evidence.started(pid)?;
            if phase == "started" {
                std::process::exit(79);
            }
            Ok(())
        },
    )
    .unwrap();
    std::process::exit(79);
}

fn complete_reconciliation_probe(operation: &mut Operation) {
    let launcher = crate::launch::test_cache().unwrap();
    let mut child = crate::process::Captured::start(
        &launcher,
        &mut std::process::Command::new("/usr/bin/true"),
        operation,
        std::time::Duration::from_secs(5),
        crate::process::Cancellation::default(),
    )
    .unwrap();
    child.output().unwrap();
}

#[test]
fn claude_namespace_is_normalized_and_keychain_account_stays_pinned() {
    use crate::state::generations::claude_namespace;
    assert_eq!(
        claude_namespace(Path::new("/example/café")).unwrap(),
        claude_namespace(Path::new("/example/cafe\u{301}")).unwrap()
    );
    let (_tmp, state, _binding) = fixture();
    let binding = state
        .register(
            "claude",
            &Profile {
                provider: Provider::Claude,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    // Claude registration may spawn `id` when USER is absent or empty.
    let _fork = crate::os::fork_guard().unwrap();
    let account = state.credential_user(&binding).unwrap().unwrap();
    let lease = state.runtime_lease(&binding).unwrap();
    let mut command = std::process::Command::new("unused");
    command.env("USER", "different");
    lease.configure(&mut command);
    assert!(
        command
            .get_envs()
            .any(|(name, value)| name == "USER" && value == Some(std::ffi::OsStr::new(&account)))
    );
}

#[test]
fn forced_writer_can_resolve_credentials_without_rewriting_cleanup_evidence() {
    use std::os::unix::process::CommandExt;
    // This case spawns probes between several exclusive-lock transitions. Run
    // it alone so other unit tests cannot inherit an OFD between fork and exec.
    if std::env::var_os("HIVE_FORCED_WRITER_FIXTURE").is_none() {
        crate::test_support::run_isolated(
            crate::test_support::rerun_isolated("state::recovery::tests::forced_writer_can_resolve_credentials_without_rewriting_cleanup_evidence", false)
                .env("HIVE_FORCED_WRITER_FIXTURE", "1"),
        );
        return;
    }
    let (_tmp, state, binding) = fixture();
    state
        .record_account(&binding, Some("original".into()))
        .unwrap();
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let receipt = operation.prepare_child(true, true).unwrap();
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(
            std::process::Command::new("/bin/sleep")
                .arg("10")
                .process_group(0),
        )
        .unwrap(),
    );
    receipt.started(child.id()).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    receipt.finish(false).unwrap();
    drop(receipt);
    drop(operation);
    let attempt = state
        .pending_credential_attempt(binding.id)
        .unwrap()
        .unwrap();
    let path = binding
        .directory
        .join("writers")
        .join(format!("{}.json", attempt.writer));
    let original = fs::read(&path).unwrap();
    {
        let mut recover = state
            .operation(&binding, crate::state::OperationKind::Recover)
            .unwrap();
        assert!(recover.confirm_account(Some("original".into())).is_err());
        complete_reconciliation_probe(&mut recover);
        assert!(recover.confirm_account(None).is_err());
    }
    assert!(!state.generations("main").unwrap()[0].credential_fence);
    assert!(state.runtime_lease(&binding).is_err());
    let mut recover = state
        .operation(&binding, crate::state::OperationKind::Recover)
        .unwrap();
    complete_reconciliation_probe(&mut recover);
    recover.confirm_account(Some("original".into())).unwrap();
    recover.finish().unwrap();
    state.runtime_lease(&binding).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    let record: ledger::Record = read_json(&path).unwrap();
    assert!(!record.confirmed && !record.quiescent);
    assert!(!ledger::conversation_proven(&record).unwrap());
}

#[test]
fn claude_index_rejects_missing_pinned_keychain_account() {
    let (tmp, _, _) = fixture();
    let state = State::new(tmp.path().join("claude")).unwrap();
    let profile = Profile {
        provider: Provider::Claude,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let binding = state.register("main", &profile).unwrap();
    let _fork = crate::os::fork_guard().unwrap();
    let path = state.root.join("index.json");
    crate::state::test_support::edit_index(&state, |index| {
        index
            .generations
            .get_mut(&binding.id)
            .unwrap()
            .keychain_account = None;
    });
    let before = fs::read(&path).unwrap();
    assert!(state.lookup("main", &profile).is_err());
    assert_eq!(fs::read(path).unwrap(), before);
}

mod cases_authentication_1;
mod cases_behaviour_1;
mod cases_indexing_1;
mod cases_recovery_1;
mod cases_recovery_2;
mod cases_requests_1;
mod cases_services_1;
mod cases_transport_1;
