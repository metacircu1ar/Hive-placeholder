//! Real child death leaves the original durable ownership intent for recovery.
use super::*;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const FIXTURE: &str = "cli::setup::skills::publication::tests::process::crash_fixture";
const PHASES: [Step; 4] = [
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
fn crash(root: &Path, phase: usize) {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FIXTURE, "--ignored", "--test-threads=1"])
            .env("HIVE_SKILL_REMOVAL_ROOT", root)
            .env("HIVE_SKILL_REMOVAL_PHASE", phase.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "owned skill removal child timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "owned child fixture invoked by death_at_unlink_and_parent_barriers_recovers_the_group"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_SKILL_REMOVAL_ROOT").unwrap());
    let phase: usize = std::env::var("HIVE_SKILL_REMOVAL_PHASE")
        .unwrap()
        .parse()
        .unwrap();
    let target = root.join("bundle/nested/doc.md");
    let _hooks = Hooks::new(move |path, step| {
        if path == target && step == PHASES[phase] && (phase == 0 || !path.exists()) {
            std::process::exit(73);
        }
        Ok(())
    });
    group(&root, b"guide", None).apply().unwrap();
    panic!("removal boundary was not reached");
}
#[test]
fn death_at_unlink_and_parent_barriers_recovers_the_group() {
    for phase in 0..PHASES.len() {
        let (_temp, root) = fixture();
        group(&root, b"guide", Some(b"doc")).apply().unwrap();
        let sentinel = root.join("personal.txt");
        fs::write(&sentinel, b"unowned").unwrap();
        crash(&root, phase);
        let target = root.join("bundle/nested/doc.md");
        assert_eq!(target.exists(), phase == 0);
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(".hive-owned.json")).unwrap()).unwrap();
        assert!(record["pending"].is_object());
        assert!(group(&root, b"guide", None).apply().unwrap());
        assert!(!group(&root, b"guide", None).apply().unwrap());
        assert!(!target.exists());
        assert_eq!(fs::read(&sentinel).unwrap(), b"unowned");
    }
}
