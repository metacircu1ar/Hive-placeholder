use super::*;
use crate::{
    config::{Method, Profile},
    test_support::{ChildGuard, rerun_isolated, wait_until},
};
use atomic_fs::{Barriers, Step};
use std::{
    fs::{self, File},
    os::unix::fs::{MetadataExt, symlink},
    path::PathBuf,
    process::Stdio,
    sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    FileBefore,
    FileAfter,
    StageBefore,
    StageAfter,
    PublishBefore,
    PublishAfter,
    ParentBefore,
    ParentAfter,
}
const BOUNDARIES: [Boundary; 8] = [
    Boundary::FileBefore,
    Boundary::FileAfter,
    Boundary::StageBefore,
    Boundary::StageAfter,
    Boundary::PublishBefore,
    Boundary::PublishAfter,
    Boundary::ParentBefore,
    Boundary::ParentAfter,
];
impl Boundary {
    fn published(self) -> bool {
        matches!(
            self,
            Self::PublishAfter | Self::ParentBefore | Self::ParentAfter
        )
    }
}
struct Fault {
    at: Boundary,
    crash: bool,
    stage_synced: AtomicBool,
}
impl Fault {
    fn new(at: Boundary, crash: bool) -> Self {
        Self {
            at,
            crash,
            stage_synced: AtomicBool::new(false),
        }
    }
    fn hit(&self, at: Boundary) -> io::Result<()> {
        if self.at != at {
            return Ok(());
        }
        if self.crash {
            // Owned subprocess crash: exit without unwinding or dropping TempDir.
            std::process::exit(73);
        }
        Err(io::Error::from_raw_os_error(libc::EIO))
    }
}
impl Barriers for Fault {
    fn observe(&self, step: Step) -> io::Result<()> {
        let at = match step {
            Step::BeforeFileSync => Boundary::FileBefore,
            Step::AfterFileSync => Boundary::FileAfter,
            Step::BeforePublish => Boundary::PublishBefore,
            Step::AfterPublish => Boundary::PublishAfter,
            Step::BeforeDirectorySync => Boundary::ParentBefore,
            Step::AfterDirectorySync => Boundary::ParentAfter,
            _ => return Ok(()),
        };
        self.hit(at)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        if self.stage_synced.swap(true, Ordering::SeqCst) {
            return file.sync_all();
        }
        self.hit(Boundary::StageBefore)?;
        file.sync_all()?;
        self.hit(Boundary::StageAfter)
    }
}
fn old(root: &Path) -> State {
    let state = State::new(root.join("old")).unwrap();
    state
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
    state
}
fn fence(destination: &Path, old: &State) {
    let bytes = fs::read(destination.join("retired-controller.json")).unwrap();
    assert!(bytes.ends_with(b"\n"));
    let saved: super::super::RetiredController = serde_json::from_slice(&bytes).unwrap();
    let expected = old.replacement_metadata(Provider::Claude).unwrap();
    assert_eq!(saved.root, expected.root);
    assert_eq!(saved.claude_namespaces, expected.claude_namespaces);
    assert!(!saved.claude_namespaces.is_empty());
    assert!(saved.metadata_unavailable.is_none());
    assert_eq!(fs::metadata(destination).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(destination.join("retired-controller.json"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
    let fresh = State::new(destination.to_owned()).unwrap();
    assert_eq!(fresh.retired_namespaces().unwrap(), saved.claude_namespaces);
}
fn snapshot(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(snapshot(&path));
        } else {
            files.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    files.sort();
    files
}
#[test]
fn faults_preserve_the_whole_publication_error_and_never_expose_an_empty_root() {
    use std::error::Error as _;
    for at in BOUNDARIES {
        let temporary = tempfile::tempdir().unwrap();
        let old = old(temporary.path());
        let before = snapshot(&old.root);
        let destination = temporary.path().join("fresh");
        let error = create(&old, &destination, Provider::Claude, &Fault::new(at, false))
            .err()
            .expect("injected failure");
        let io = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
        let publication = io
            .get_ref()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .unwrap();
        assert_eq!(publication.published(), at.published(), "{at:?}");
        assert_eq!(publication.io_error().raw_os_error(), Some(libc::EIO));
        assert!(publication.cleanup_error().is_none());
        assert!(publication.retained().is_none());
        assert_eq!(destination.exists(), at.published(), "{at:?}");
        if at.published() {
            fence(&destination, &old);
            assert!(
                old.replacement_root(&destination, Provider::Claude)
                    .is_err()
            );
            fence(&destination, &old);
        }
        assert_eq!(snapshot(&old.root), before);
        assert_eq!(
            fs::read_dir(temporary.path()).unwrap().count(),
            // Old root + its initialization/admission locks + destination admission.
            4 + usize::from(at.published())
        );
    }
}
#[test]
fn crashes_at_each_barrier_leave_absence_or_a_complete_fence_and_keep_old_state() {
    for (index, at) in BOUNDARIES.into_iter().enumerate() {
        let temporary = tempfile::tempdir().unwrap();
        let old = old(temporary.path());
        let before = snapshot(&old.root);
        let destination = temporary.path().join("fresh");
        let mut command =
            rerun_isolated("state::controller::publication::tests::crash_fixture", true);
        command
            .env("HIVE_CONTROLLER_CRASH_ROOT", temporary.path())
            .env("HIVE_CONTROLLER_CRASH_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "fixture did not reach {at:?}");
        assert_eq!(destination.exists(), at.published());
        if at.published() {
            fence(&destination, &old);
        } else {
            // A private abandoned stage is not a published controller. Retry does
            // not sweep it or any unrelated entries in the destination parent.
            let stages: Vec<_> = fs::read_dir(temporary.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|p| p != &old.root && p.is_dir())
                .collect();
            assert_eq!(stages.len(), 1);
            old.replacement_root(&destination, Provider::Claude)
                .unwrap();
            fence(&destination, &old);
            assert!(stages[0].is_dir());
        }
        assert_eq!(snapshot(&old.root), before);
    }
}
#[test]
#[ignore = "owned subprocess fixture; exercised at all boundaries by crashes_at_each_barrier"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_CONTROLLER_CRASH_ROOT").unwrap());
    let index: usize = std::env::var("HIVE_CONTROLLER_CRASH_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let old = State::new(root.join("old")).unwrap();
    create(
        &old,
        &root.join("fresh"),
        Provider::Claude,
        &Fault::new(BOUNDARIES[index], true),
    )
    .unwrap();
    panic!("crash barrier was not reached");
}

#[test]
fn no_replace_commit_has_one_winner_even_after_both_prechecks() {
    struct Race(Barrier);
    impl Barriers for Race {
        fn observe(&self, step: Step) -> io::Result<()> {
            if step == Step::BeforePublish {
                self.0.wait();
            }
            Ok(())
        }
    }
    let temporary = tempfile::tempdir().unwrap();
    let old = old(temporary.path());
    let before = snapshot(&old.root);
    let destination = temporary.path().join("fresh");
    let race = Race(Barrier::new(2));
    let wins = std::thread::scope(|scope| {
        let a = scope.spawn(|| create(&old, &destination, Provider::Claude, &race).is_ok());
        let b = scope.spawn(|| create(&old, &destination, Provider::Claude, &race).is_ok());
        usize::from(a.join().unwrap()) + usize::from(b.join().unwrap())
    });
    assert_eq!(wins, 1);
    fence(&destination, &old);
    assert_eq!(snapshot(&old.root), before);
    let collision = temporary.path().join("symlink");
    symlink(&destination, &collision).unwrap();
    assert!(old.replacement_root(&collision, Provider::Claude).is_err());
    fence(&destination, &old);
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 6);
}

struct Observe {
    phase: std::sync::mpsc::SyncSender<bool>,
    observed: std::sync::mpsc::Receiver<()>,
}
impl Barriers for Observe {
    fn observe(&self, step: Step) -> io::Result<()> {
        if matches!(step, Step::BeforePublish | Step::AfterPublish) {
            self.phase
                .send(step == Step::AfterPublish)
                .map_err(io::Error::other)?;
            self.observed
                .recv_timeout(Duration::from_secs(10))
                .map_err(io::Error::other)?;
        }
        Ok(())
    }
}

#[test]
fn concurrent_readers_never_observe_an_unfenced_published_root() {
    let temporary = tempfile::tempdir().unwrap();
    let old = old(temporary.path());
    let before = snapshot(&old.root);
    let destinations: Vec<_> = (0..32)
        .map(|i| temporary.path().join(format!("fresh-{i}")))
        .collect();
    let done = AtomicBool::new(false);
    let (phase, phases) = std::sync::mpsc::sync_channel(1);
    let (observed, observations) = std::sync::mpsc::sync_channel(1);
    let barrier = Observe {
        phase,
        observed: observations,
    };
    std::thread::scope(|scope| {
        let (old, destinations, done) = (&old, &destinations, &done);
        let reader = scope.spawn(move || {
            assert!(!phases.recv_timeout(Duration::from_secs(10)).unwrap());
            assert!(!destinations[0].exists());
            observed.send(()).unwrap();
            assert!(phases.recv_timeout(Duration::from_secs(10)).unwrap());
            fence(&destinations[0], old);
            assert!(!done.load(Ordering::Acquire));
            observed.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                for path in destinations {
                    match fs::metadata(path) {
                        Ok(_) => fence(path, old),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                        Err(error) => panic!("reader metadata: {error}"),
                    }
                }
                if done.load(Ordering::Acquire) {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        });
        create(old, &destinations[0], Provider::Claude, &barrier).unwrap();
        for path in &destinations[1..] {
            old.replacement_root(path, Provider::Claude).unwrap();
        }
        done.store(true, Ordering::Release);
        reader.join().unwrap();
    });
    for path in &destinations {
        fence(path, &old);
    }
    assert_eq!(snapshot(&old.root), before);
}
