use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use atomic_fs::{Barriers, Step};
use std::{fs, io, path::PathBuf, process::Stdio, time::Duration};

const STEPS: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
fn published(step: Step) -> bool {
    matches!(
        step,
        Step::AfterPublish | Step::BeforeDirectorySync | Step::AfterDirectorySync
    )
}
struct Die(Step);
impl Barriers for Die {
    fn observe(&self, step: Step) -> io::Result<()> {
        if step == self.0 {
            std::process::exit(73);
        }
        Ok(())
    }
}

#[test]
fn process_death_at_stream_publication_barriers_never_exposes_partial_output() {
    for (index, step) in STEPS.into_iter().enumerate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output");
        let mut command = rerun_isolated("publication::tests::death::crash_fixture", true);
        command
            .env("HIVE_STREAM_PATH", &path)
            .env("HIVE_STREAM_STEP", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73));
        assert_eq!(path.exists(), published(step));
        let retained = fs::read_dir(root.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(retained).unwrap(), b"first second");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
#[ignore = "owned subprocess fixture; exercised at every barrier by process_death_at_stream_publication_barriers_never_exposes_partial_output"]
fn crash_fixture() {
    let path = PathBuf::from(std::env::var_os("HIVE_STREAM_PATH").unwrap());
    let index: usize = std::env::var("HIVE_STREAM_STEP").unwrap().parse().unwrap();
    atomic_fs::PreparedCreate::new(&path, hive_setup_support::names::PUBLICATION_OPTIONS)
        .unwrap()
        .publish_stream(
            |output| {
                output.write_all(b"first ")?;
                output.write_all(b"second")
            },
            &Die(STEPS[index]),
        )
        .unwrap();
    panic!("stream crash fixture missed its barrier");
}
