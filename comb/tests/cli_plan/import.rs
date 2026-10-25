use comb::Comb;
use comb_protocol::import::{ImportProgress, ImportState, ImportStatus};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Owned(Option<Child>);
impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0
            && child.try_wait().unwrap().is_none()
        {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }
}
impl Owned {
    fn spawn(root: &Path, args: &[&std::ffi::OsStr]) -> Self {
        Self(Some(
            Command::new(env!("CARGO_BIN_EXE_comb"))
                .arg("--root")
                .arg(root)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }
    fn finish(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.0.as_mut().unwrap().try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "owned import CLI timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
fn cli(root: &Path, args: &[&str]) -> Output {
    Owned::spawn(
        root,
        &args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>(),
    )
    .finish()
}
fn status(root: &Path) -> ImportProgress {
    let output = cli(root, &["import", "--status"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<ImportStatus>(&output.stdout)
        .unwrap()
        .current
        .unwrap()
}

#[test]
fn import_crash_before_reply_can_be_discovered_and_abandoned_without_source() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let source = tempfile::tempdir().unwrap();
    drop(Comb::init(root.path()).unwrap());
    // Enough bounded work that acceptance precedes the final reply by many pages.
    for index in 0..1000 {
        fs::write(
            source.path().join(format!("page-{index:04}.md")),
            "Private import fact",
        )
        .unwrap();
    }
    let mut child = Owned::spawn(root.path(), &["import".as_ref(), source.path().as_os_str()]);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.path().join(".comb/import.json").exists() {
        assert!(
            child.0.as_mut().unwrap().try_wait().unwrap().is_none(),
            "import exited before acceptance"
        );
        assert!(Instant::now() < deadline, "import acceptance timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
    child.0.as_mut().unwrap().kill().unwrap();
    let output = child.finish();
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "crash must precede the import reply"
    );
    let before = status(root.path());
    assert_eq!(before.state, ImportState::Pending);
    assert_eq!(before.total, 1000);
    // Exercise the real CLI error envelope on a resumed source conflict.
    fs::write(source.path().join("page-0999.md"), "Changed source").unwrap();
    let failed =
        Owned::spawn(root.path(), &["import".as_ref(), source.path().as_os_str()]).finish();
    assert_eq!(failed.status.code(), Some(1));
    let reply: serde_json::Value = serde_json::from_slice(&failed.stdout).unwrap();
    let error: comb::Error = serde_json::from_value(reply["error"].clone()).unwrap();
    assert!(error.file_committed);
    assert_eq!(error.import_progress.unwrap().batch_id, before.batch_id);
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("Changed source"));
    fs::remove_dir_all(source.path()).unwrap();
    assert_eq!(status(root.path()), before);
    let abandoned = cli(
        root.path(),
        &["import", "--abandon", before.batch_id.as_str()],
    );
    assert!(abandoned.status.success());
    let report: ImportProgress = serde_json::from_slice(&abandoned.stdout).unwrap();
    assert_eq!(report.state, ImportState::Abandoned);
    assert_eq!(report.completed, before.completed);
    assert_eq!(
        cli(
            root.path(),
            &["import", "--abandon", before.batch_id.as_str()]
        )
        .stdout,
        abandoned.stdout
    );
}
