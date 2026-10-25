use super::*;
use crate::{
    config::{Method, Profile, Provider},
    state::{
        Binding, Generation, GenerationState, ProcessIdentity, State, test_support::edit_index,
    },
};
use std::{cell::RefCell, fs, rc::Rc};

const STEPS: [Step; 4] = [
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
type Hook = Box<dyn Fn(Step) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
pub(super) fn step(value: Step) -> io::Result<()> {
    HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(value)))
}
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
fn with_hook<T>(hook: impl Fn(Step) -> io::Result<()> + 'static, run: impl FnOnce() -> T) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
fn fail<T>(at: Step, run: impl FnOnce() -> Result<T>) -> Error {
    let result = with_hook(
        move |step| {
            if step == at {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        run,
    );
    let error = result.err().expect("injected sync failure");
    assert_eq!(error.code(), "state_io");
    assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
    error
}
fn profile() -> Profile {
    Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    }
}
fn fixture() -> (tempfile::TempDir, State, Binding) {
    let temporary = tempfile::tempdir().unwrap();
    let state = State::new(temporary.path().join("state")).unwrap();
    let binding = state.register("main", &profile()).unwrap();
    (temporary, state, binding)
}
fn saved(state: &State, binding: &Binding) -> Generation {
    state.read_index().unwrap().generations[&binding.id].clone()
}
fn assert_identity(state: &State, binding: &Binding) {
    let stored = saved(state, binding);
    assert!(stored.binding.ready);
    assert_eq!(
        stored.credential_lock,
        Some(LockIdentity::at(&binding.directory.join("auth.lock")).unwrap())
    );
}
fn unfinished(generation: &Generation) {
    assert!(!generation.binding.ready);
    assert!(generation.credential_lock.is_none());
    assert!(!generation.binding.directory.join("executions").exists());
    assert!(
        !generation
            .binding
            .directory
            .join("prune-receipt.json")
            .exists()
    );
    for directory in ["provider", "work"] {
        assert_eq!(
            fs::read_dir(generation.binding.directory.join(directory))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn registration_sync_failures_keep_pending_identity_and_prior_profile_unchanged() {
    let _fork = crate::os::fork_guard().unwrap();
    for at in STEPS {
        let (_temporary, state, active) = fixture();
        let before = serde_json::to_value(saved(&state, &active)).unwrap();
        fail(at, || state.register("second", &profile()));
        let index = state.read_index().unwrap();
        let pending = index.profiles["second"].clone();
        assert_eq!(index.generations.len(), 2);
        unfinished(&index.generations[&pending.id]);
        assert_eq!(
            serde_json::to_value(saved(&state, &active)).unwrap(),
            before
        );
        let identity = LockIdentity::at(&pending.directory.join("auth.lock")).unwrap();
        let ready = state.register("second", &profile()).unwrap();
        assert_eq!(ready.id, pending.id);
        assert_identity(&state, &ready);
        assert_eq!(
            LockIdentity::at(&ready.directory.join("auth.lock")).unwrap(),
            identity
        );
    }
}

#[test]
fn repair_sync_failures_keep_unready_candidate_and_do_not_cut_over() {
    let _fork = crate::os::fork_guard().unwrap();
    for at in STEPS {
        let (_temporary, state, active) = fixture();
        let before = serde_json::to_value(saved(&state, &active)).unwrap();
        fail(at, || state.begin_repair("main", &profile()));
        let index = state.read_index().unwrap();
        assert_eq!(index.profiles["main"].id, active.id);
        assert_eq!(index.generations.len(), 2);
        let pending = index
            .generations
            .values()
            .find(|g| g.binding.id != active.id)
            .unwrap();
        assert_eq!(pending.state, GenerationState::Candidate);
        assert_eq!(pending.replaces, Some(active.id));
        unfinished(pending);
        assert_eq!(
            serde_json::to_value(saved(&state, &active)).unwrap(),
            before
        );
        // A new repair uses a separate candidate; the failed intent is retained.
        let next = state.begin_repair("main", &profile()).unwrap();
        assert_ne!(next.binding.id, pending.binding.id);
        assert_identity(&state, &next.binding);
        unfinished(&saved(&state, &pending.binding));
    }
}

fn dead_owner() -> ProcessIdentity {
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(std::process::Command::new("/bin/sleep").arg("10")).unwrap(),
    );
    let owner = ProcessIdentity::read(child.id()).unwrap().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    owner
}
#[test]
fn abandoned_prune_sync_failures_do_not_commit_ready_or_pruning_state() {
    let owner = dead_owner();
    let _fork = crate::os::fork_guard().unwrap();
    for at in STEPS {
        let (_temporary, state, active) = fixture();
        let repair = state.begin_repair("main", &profile()).unwrap();
        edit_index(&state, |index| {
            let pending = index.generations.get_mut(&repair.binding.id).unwrap();
            pending.repair_owner = Some(owner.clone());
            pending.binding.ready = false;
            pending.credential_lock = None;
            fs::remove_file(pending.binding.directory.join("auth.lock")).unwrap();
            crate::state::write_json(
                &pending.binding.directory.join("identity.json"),
                &pending.binding,
            )
            .unwrap();
        });
        let index_before = fs::read(state.root.join("index.json")).unwrap();
        let active_before = serde_json::to_value(saved(&state, &active)).unwrap();
        fail(at, || state.prune("main", repair.binding.id));
        assert_eq!(
            fs::read(state.root.join("index.json")).unwrap(),
            index_before
        );
        let pending = saved(&state, &repair.binding);
        assert_eq!(pending.state, GenerationState::Candidate);
        unfinished(&pending);
        let identity = LockIdentity::at(&pending.binding.directory.join("auth.lock")).unwrap();
        state.prune("main", repair.binding.id).unwrap();
        assert_eq!(
            saved(&state, &repair.binding).state,
            GenerationState::Pruned
        );
        assert_identity(&state, &repair.binding);
        assert_eq!(
            LockIdentity::at(&pending.binding.directory.join("auth.lock")).unwrap(),
            identity
        );
        assert_eq!(
            serde_json::to_value(saved(&state, &active)).unwrap(),
            active_before
        );
    }
}

#[test]
fn durable_acquisition_holds_one_inode_through_ordered_barriers_and_rejects_replacement() {
    let _fork = crate::os::fork_guard().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("auth.lock");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let check = path.clone();
    let events = seen.clone();
    let held = with_hook(
        move |step| {
            events.borrow_mut().push(step);
            assert_eq!(
                Lock::acquire_existing(&check).err().unwrap().code(),
                "profile_busy"
            );
            Ok(())
        },
        || Lock::acquire_durable(&path).unwrap(),
    );
    assert_eq!(&*seen.borrow(), &STEPS);
    let identity = LockIdentity::from_fd(held.fd()).unwrap();
    assert_eq!(LockIdentity::at(&path).unwrap(), identity);
    drop(held);
    let held = Lock::acquire_durable(&path).unwrap();
    assert_eq!(LockIdentity::from_fd(held.fd()).unwrap(), identity);
    drop(held);
    let check = path.clone();
    let retained = temporary.path().join("retained");
    let error = with_hook(
        move |step| {
            if step == Step::AfterDirectorySync {
                fs::rename(&check, &retained)?;
                drop(crate::state::private_file(&check, true).unwrap());
            }
            Ok(())
        },
        || Lock::acquire_durable(&path),
    )
    .err()
    .expect("replacement rejected");
    assert_eq!(error.code(), "unsafe_state");
    assert!(path.exists());
    assert!(temporary.path().join("retained").exists());
}
