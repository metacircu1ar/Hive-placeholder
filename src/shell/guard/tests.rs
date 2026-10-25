use super::*;
mod publication;

#[test]
fn helper_failures_relay_only_fixed_metadata_and_never_confirm_cleanup() {
    use crate::shell::diagnostics::{Diagnostics, Phase};
    const CHILD: &str = "HIVE_SHELL_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut command = crate::test_support::rerun_isolated(
            &format!(
                "{module}::helper_failures_relay_only_fixed_metadata_and_never_confirm_cleanup"
            ),
            false,
        );
        crate::test_support::run_isolated(command.env(CHILD, "1"));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagnostics.jsonl");
    let config = hive_log::Config {
        level: tracing::Level::DEBUG.into(),
        ..Default::default()
    };
    let (logger, drain) = hive_log::start(&path, config).unwrap();
    logger.install_global().unwrap();
    let mut expected = Diagnostics::default();
    expected.record(Phase::Signal, &io::Error::from_raw_os_error(libc::ESRCH));
    expected.log(std::process::id());
    let mut guard = ShellGuard::create(dir.path(), "private-instructions-canary", || {
        std::env::current_exe().map_err(Error::state_io_from)
    })
    .unwrap();
    let token = guard.expect().unwrap();
    guard.expected.insert(token.clone(), true);
    let mut peer = accepted(&mut guard, Some(token.clone()));
    let mut diagnostics = Diagnostics::default();
    for _ in 0..1000 {
        diagnostics.record(Phase::Signal, &io::Error::from_raw_os_error(libc::EPERM));
    }
    diagnostics.record(Phase::Signal, &io::Error::from_raw_os_error(libc::ESRCH));
    diagnostics.record(
        Phase::Operation,
        &io::Error::other("private-cleanup-canary"),
    );
    let message = super::super::Envelope::new(Request::Failed { diagnostics });
    let mut bytes = serde_json::to_vec(&message).unwrap();
    assert!(bytes.len() < 256);
    bytes.push(b'\n');
    peer.write_all(&bytes).unwrap();
    guard.service().unwrap();
    assert!(!guard.uncertain);
    guard.connections[0].lost_at =
        Some(Instant::now().checked_sub(Duration::from_secs(4)).unwrap());
    assert!(guard.service().is_err());
    assert!(guard.uncertain);
    assert!(!guard.clean());
    assert_eq!(guard.expected.get(&token), Some(&true));
    assert_eq!(
        drain.finish(Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = fs::read_to_string(path).unwrap();
    assert!(!text.contains("private-"));
    assert!(!text.contains(&token));
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        rows.iter()
            .any(|row| row["event"] == "shell.cleanup_uncertain")
    );
    let signal = rows
        .iter()
        .find(|row| row["phase"] == "signal" && row["level"] == "warn")
        .unwrap();
    assert_eq!(signal["count"], 1001);
    assert_eq!(signal["errno"], libc::EPERM);
    assert!(
        rows.iter()
            .any(|row| row["event"] == "shell.helper_already_exited" && row["level"] == "debug")
    );
    assert!(rows.len() <= 4);
}

fn accepted(guard: &mut ShellGuard, token: Option<String>) -> UnixStream {
    let (stream, peer) = UnixStream::pair().unwrap();
    stream.set_nonblocking(true).unwrap();
    let pid = std::process::id();
    guard.connections.push(Connection {
        stream,
        peer: os::shell::Peer {
            pid,
            local_pid: pid,
        },
        identity: ProcessIdentity::read(pid).unwrap().unwrap(),
        buffer: Vec::new(),
        phase: token.map_or(ConnectionPhase::AwaitingRegistration, |token| {
            ConnectionPhase::Registered(Registration {
                token,
                remote_fd: std::os::fd::AsRawFd::as_raw_fd(&peer),
            })
        }),
        deadline: Instant::now() + Duration::from_secs(10),
        lost_at: None,
        output: Vec::new(),
    });
    peer
}

#[test]
fn malformed_registered_message_keeps_parser_cause_and_uncertain_cleanup() {
    use std::error::Error as _;

    let dir = tempfile::tempdir().unwrap();
    let mut guard = ShellGuard::create(dir.path(), "test", || {
        std::env::current_exe().map_err(Error::state_io_from)
    })
    .unwrap();
    let token = guard.expect().unwrap();
    guard.expected.insert(token.clone(), true);
    let mut peer = accepted(&mut guard, Some(token.clone()));
    peer.write_all(b"malformed\n").unwrap();
    let error = guard.service().unwrap_err();
    assert!(guard.uncertain);
    assert!(!guard.clean());
    assert_eq!(guard.expected.get(&token), Some(&true));
    assert!(matches!(error.source(), Some(source) if source.is::<serde_json::Error>()));
    assert_eq!(
        crate::runtime::api::value(error),
        serde_json::json!({
            "code":"provider_protocol_error","message":"Shell supervision failed."
        })
    );
}

#[test]
fn accepted_stray_and_expired_idle_connections_leave_registered_work_intact() {
    let dir = tempfile::tempdir().unwrap();
    let mut guard = ShellGuard::create(dir.path(), "test", || {
        std::env::current_exe().map_err(Error::state_io_from)
    })
    .unwrap();
    let token = guard.expect().unwrap();
    guard.expected.insert(token.clone(), true);
    let _registered = accepted(&mut guard, Some(token.clone()));
    let mut stray = accepted(&mut guard, None);
    stray.write_all(b"malformed\n").unwrap();
    let _idle = accepted(&mut guard, None);
    guard.connections.last_mut().unwrap().deadline =
        Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    guard.service().unwrap();
    assert!(!guard.uncertain);
    assert_eq!(guard.connections.len(), 1);
    assert_eq!(guard.expected.get(&token), Some(&true));
    assert!(!guard.unregistered(&token));
    assert!(!guard.clean());
}

#[test]
fn workspace_rejection_precedes_lazy_binary_resolution() {
    let cwd = Path::new("/tmp").canonicalize().unwrap();
    let resolutions = std::cell::Cell::new(0);
    let result = ShellGuard::create(&cwd, "test", || {
        resolutions.set(resolutions.get() + 1);
        Err(Error::invalid_executable("fixture binary unavailable"))
    });
    assert!(matches!(result, Err(Error::InvalidRequest { .. })));
    assert_eq!(resolutions.get(), 0);
}

#[test]
fn lost_registered_helper_without_attachment_cannot_prove_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let mut guard = ShellGuard::create(dir.path(), "test", || {
        std::env::current_exe().map_err(Error::state_io_from)
    })
    .unwrap();
    let token = guard.expect().unwrap();
    guard.expected.insert(token.clone(), true);
    let peer = accepted(&mut guard, Some(token.clone()));
    drop(peer);
    guard.service().unwrap();
    assert_eq!(guard.fds().len(), 1); // Only listener, no permanently-ready HUP.
    assert_eq!(
        guard.wait_limit(Duration::from_secs(10)),
        Duration::from_millis(250)
    );
    guard.connections[0].lost_at =
        Some(Instant::now().checked_sub(Duration::from_secs(4)).unwrap());
    assert!(guard.service().is_err());
    assert!(guard.uncertain);
    guard.seal();
    assert!(!guard.sealed_clean());
    assert_eq!(guard.expected.get(&token), Some(&true));
}
#[test]
fn accept_backoff_keeps_existing_connections_in_poll_and_bounds_wakeup() {
    let dir = tempfile::tempdir().unwrap();
    let mut guard = ShellGuard::create(dir.path(), "test", || {
        std::env::current_exe().map_err(Error::state_io_from)
    })
    .unwrap();
    let _peer = accepted(&mut guard, None);
    assert_eq!(guard.fds().len(), 2);
    guard.accept_after = Some(Instant::now() + Duration::from_millis(250));
    assert_eq!(guard.fds().len(), 1);
    assert!(guard.wait_limit(Duration::from_secs(10)) <= Duration::from_millis(250));
    guard.service().unwrap();
    assert!(!guard.uncertain);
    assert_eq!(guard.connections.len(), 1);
    guard.accept_after = Some(Instant::now());
    guard.service().unwrap();
    assert_eq!(guard.fds().len(), 2);
    assert_eq!(
        guard.wait_limit(Duration::from_secs(10)),
        Duration::from_millis(250)
    );
}
