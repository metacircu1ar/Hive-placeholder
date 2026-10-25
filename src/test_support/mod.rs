//! Shared deterministic observation and subprocess setup for in-crate tests.
use std::{
    io::Read,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Snapshot tests may allow an absent artifact, never an unreadable one.
pub(crate) fn optional_bytes(path: impl AsRef<std::path::Path>) -> Option<Vec<u8>> {
    match std::fs::read(path.as_ref()) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!(
            "cannot snapshot test artifact {}: {error}",
            path.as_ref().display()
        ),
    }
}

pub(crate) fn wait_until<T>(
    timeout: Duration,
    interval: Duration,
    mut observe: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = observe() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "test observation timed out after {timeout:?}"
        );
        std::thread::sleep(interval);
    }
}

/// Build a selector; normal success must be checked with `run_isolated`.
pub(crate) fn rerun_isolated(name: &str, fixture: bool) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", name, "--nocapture"]);
    if fixture {
        command.arg("--ignored");
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("HIVE_") {
            command.env_remove(name);
        }
    }
    command
}

pub(crate) fn run_isolated(command: &mut Command) {
    isolated_output(command).assert_success();
}

struct IsolatedOutput {
    status: ExitStatus,
    stdout: String,
}
impl IsolatedOutput {
    fn assert_success(&self) {
        let summary = self
            .stdout
            .lines()
            .rfind(|line| line.starts_with("test result: "));
        assert!(
            self.status.success()
                && summary.is_some_and(|line| line
                    .starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; ")),
            "isolated test did not execute exactly one successful test: {}; stdout:\n{}",
            self.status,
            self.stdout
        );
    }
}

fn isolated_output(command: &mut Command) -> IsolatedOutput {
    // Drain stdout before waiting, retaining kill/reap ownership on read failures.
    let mut child = ChildGuard(crate::os::spawn(command.stdout(Stdio::piped())).unwrap());
    assert!(
        child.stderr.is_none(),
        "isolated test stderr must not be piped"
    );
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let status = child.wait().unwrap();
    IsolatedOutput { status, stdout }
}

macro_rules! assert_code {
    ($result:expr, $code:expr $(,)?) => {
        match $result {
            Err(error) => assert_eq!(error.code(), $code),
            Ok(_) => panic!("expected error code {}", $code),
        }
    };
}
pub(crate) use assert_code;

pub(crate) struct ChildGuard(pub std::process::Child);
impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests;
