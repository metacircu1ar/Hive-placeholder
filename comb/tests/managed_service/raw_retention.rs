use super::*;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

fn raw(fixture: &Fixture) -> File {
    files::directory(fixture.spec.log.parent().unwrap()).unwrap();
    OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o600)
        .open(&fixture.spec.log)
        .unwrap()
}

fn flood(file: &mut File, marker: &[u8]) {
    file.write_all(&vec![
        b'x';
        usize::try_from(hive_log::raw::TRIM_BYTES).unwrap() + 1
    ])
    .unwrap();
    file.write_all(marker).unwrap();
}

fn previous(fixture: &Fixture) -> PathBuf {
    let mut path = fixture.spec.log.as_os_str().to_owned();
    path.push(".prev");
    PathBuf::from(path)
}

fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "managed retention deadline exceeded"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn managed_native_log_trims_before_ready_and_after_flood_without_replacing_inode() {
    let f = Fixture::new();
    let mut native = raw(&f);
    let inode = native.metadata().unwrap().ino();
    flood(&mut native, b"before-entry-native-marker");
    let mut child = Running(
        f.command()
            .stdout(Stdio::null())
            .stderr(native.try_clone().unwrap())
            .spawn()
            .unwrap(),
    );
    until(|| {
        assert!(child.0.try_wait().unwrap().is_none());
        comb::services::health(&f.root, true).unwrap().is_some()
    });
    assert!(
        std::fs::read(previous(&f))
            .unwrap()
            .ends_with(b"before-entry-native-marker")
    );
    assert!(native.metadata().unwrap().len() < 8192);
    flood(&mut native, b"after-ready-native-marker");
    until(|| native.metadata().unwrap().len() < 8192);
    let tail = std::fs::read(previous(&f)).unwrap();
    assert_eq!(tail.len(), hive_log::raw::TAIL_BYTES);
    assert!(tail.ends_with(b"after-ready-native-marker"));
    assert_eq!(std::fs::metadata(&f.spec.log).unwrap().ino(), inode);
    native.write_all(b"append-survives-truncation").unwrap();
    assert!(
        std::fs::read(&f.spec.log)
            .unwrap()
            .ends_with(b"append-survives-truncation")
    );
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child.0),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    until(|| {
        child.0.try_wait().unwrap().is_some_and(|status| {
            assert!(status.success());
            true
        })
    });
}

#[test]
fn managed_environment_failure_keeps_native_tail_and_suppression_evidence() {
    let f = Fixture::new();
    let mut native = raw(&f);
    flood(&mut native, b"loader-native-marker");
    files::write(
        f.spec.environment_file.as_ref().unwrap(),
        b"invalid environment",
        true,
    )
    .unwrap();
    let output = f
        .command()
        .stderr(native.try_clone().unwrap())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        std::fs::read(previous(&f))
            .unwrap()
            .ends_with(b"loader-native-marker")
    );
    let failure = managed::failure(&f.spec).unwrap().unwrap();
    assert!(failure.suppressed);
    assert_eq!(failure.code, "service_environment_invalid");
    assert!(!f.root.join(".comb/logs").exists());
    let after = std::fs::read(&f.spec.log).unwrap();
    assert!(after.len() < 8192);
    assert!(!String::from_utf8_lossy(&after).contains("invalid environment"));
}

#[test]
fn invalid_arguments_do_not_admit_or_trim_raw_output() {
    let f = Fixture::new();
    let mut native = raw(&f);
    flood(&mut native, b"pre-admission-native-marker");
    let before = std::fs::read(&f.spec.log).unwrap();
    let output = f
        .command()
        .arg("--unknown-option")
        .stderr(native)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!previous(&f).exists());
    assert!(std::fs::read(&f.spec.log).unwrap().starts_with(&before));
    let name = f.spec.log.file_name().unwrap().to_str().unwrap();
    assert!(
        !f.spec
            .log
            .with_file_name(format!(".{name}.raw-owner.lock"))
            .exists()
    );
    assert!(!f.root.join(".comb/logs").exists());
}
