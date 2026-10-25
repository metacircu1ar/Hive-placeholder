use super::*;
use crate::{disk::publication_hooks as hooks, service::auth::ConnectionExt as _};
use hive_setup_support::{
    files,
    runner::{Output, Runner},
    service::{Controller, Layout, Platform, Scope, Spec},
};
use std::{cell::Cell, error::Error as _, io, sync::mpsc, time::Duration};

pub(super) fn probe(root: &Path, path: &Path) -> Result<Option<serde_json::Value>> {
    let connection = crate::service::auth::Connection::human(root, path.into())?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(crate::service::client::try_call(
            &connection,
            &crate::service::Operation::Health,
            None,
            Duration::from_secs(1),
        ))
}

#[test]
fn configured_listener_preserves_descriptor_defaults_and_unsafe_listener_is_refused() {
    let (root, path) = fixture();
    let (listener, mut guard) = Lease::acquire(root.path(), path.clone())
        .unwrap()
        .bind()
        .unwrap();
    assert!(
        rustix::io::fcntl_getfd(&listener)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    assert!(
        rustix::fs::fcntl_getfl(&listener)
            .unwrap()
            .contains(rustix::fs::OFlags::NONBLOCK)
    );
    #[cfg(target_vendor = "apple")]
    assert!(rustix::net::sockopt::socket_nosigpipe(&listener).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        probe(root.path(), &path).unwrap_err().code,
        crate::ErrorCode::UnsafeSocket
    );
    drop(listener);
    // Refusal depends on the unsafe mode, not filesystem timestamp precision.
    assert!(guard.cleanup().is_err());
    assert!(path.exists());
}

#[test]
fn post_bind_failures_retain_native_cause_and_clean_only_the_captured_socket() {
    for boundary in [atomic_fs::Step::Written, atomic_fs::Step::BeforePublish] {
        let (root, path) = fixture();
        let selected = path.clone();
        let error = hooks::with(
            move |at, step| {
                if at == selected && step == boundary {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || {
                Lease::acquire(root.path(), path.clone())
                    .unwrap()
                    .bind()
                    .err()
                    .unwrap()
            },
        );
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::EIO)
        );
        assert!(!path.exists());
        assert!(!receipt_path(&path).exists());
        drop(Lease::acquire(root.path(), path).unwrap());
    }
}

#[test]
fn failed_bind_preserves_a_foreign_socket() {
    let (root, path) = fixture();
    let foreign = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let before = Node::of(&rustix::fs::stat(&path).unwrap());
    assert!(listener::bound(&path).is_err());
    assert_eq!(Node::of(&rustix::fs::stat(&path).unwrap()), before);
    assert!(path.exists());
    drop(foreign);
    drop(root);
}

struct Starting {
    start: mpsc::Sender<()>,
    bound: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<std::thread::JoinHandle<Result<()>>>,
    started: Cell<bool>,
}
impl Starting {
    fn new(root: PathBuf, path: PathBuf) -> Self {
        let (start, begin) = mpsc::channel();
        let (ready, bound) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let task = std::thread::spawn(move || {
            begin
                .recv_timeout(Duration::from_secs(10))
                .map_err(io::Error::other)?;
            let selected = path.clone();
            let server = hooks::with(
                move |at, step| {
                    if at == selected && step == atomic_fs::Step::Written {
                        // Reproduce permissive creation regardless of this test
                        // process's umask, without changing that global setting.
                        fs::set_permissions(at, fs::Permissions::from_mode(0o777))?;
                        ready.send(()).map_err(io::Error::other)?;
                        resume
                            .recv_timeout(Duration::from_secs(10))
                            .map_err(io::Error::other)?;
                    }
                    Ok(())
                },
                || {
                    Server::open(
                        &root,
                        Options {
                            socket: Some(path),
                            ..Options::default()
                        },
                    )
                },
            )?;
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(server.run(async {
                    let _ = shutdown.await;
                }))
        });
        Self {
            start,
            bound,
            release,
            stop: Some(stop),
            task: Some(task),
            started: Cell::new(false),
        }
    }
    fn finish(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.task.take().unwrap().join().unwrap().unwrap();
    }
}
impl Drop for Starting {
    fn drop(&mut self) {
        let _ = self.start.send(());
        let _ = self.release.send(());
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let joined = task.join();
            if !std::thread::panicking() {
                assert!(joined.is_ok(), "socket fixture worker panicked");
            }
        }
    }
}
impl Runner for Starting {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
        assert_eq!(program, "/usr/bin/systemctl");
        assert_eq!(args[0], "--user");
        match args[1].as_str() {
            "show" => assert_eq!(args[2], "--property=Version"),
            "reset-failed" => (),
            "start" => {
                assert!(!self.started.replace(true));
                self.start.send(()).map_err(io::Error::other)?;
                self.bound
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(io::Error::other)?;
            }
            other => panic!("unexpected manager operation {other}"),
        }
        Ok(Output {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[test]
fn controller_start_waits_across_the_real_bound_before_listen_boundary() {
    let (root, path) = fixture();
    let layout = Layout {
        home: root.path().into(),
        config: root.path().join("cfg"),
        state: root.path().join("state"),
        platform: Platform::Systemd,
    };
    let scope = Scope::new("comb", &[root.path()]).unwrap();
    let spec = Spec::new(
        &layout,
        scope.clone(),
        Path::new("/bin/echo"),
        vec!["serve".into()],
        None,
        vec![],
    )
    .unwrap();
    files::write(&spec.descriptor, &files::encode(&spec).unwrap(), false).unwrap();
    files::write(&spec.unit, &spec.render().unwrap(), false).unwrap();
    let runner = Starting::new(root.path().into(), path.clone());
    let released = Cell::new(false);
    let result = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    }
    .start(&|| {
        let health = probe(root.path(), &path).map_err(io::Error::other)?;
        if runner.started.get() && !released.replace(true) {
            assert!(health.is_none());
            assert_eq!(
                comb_protocol::service::check_socket(&path)
                    .unwrap_err()
                    .code,
                crate::ErrorCode::UnsafeSocket
            );
            runner.release.send(()).map_err(io::Error::other)?;
        }
        Ok(health.is_some())
    });
    assert!(released.get());
    let health = probe(root.path(), &path).unwrap().unwrap();
    let _: comb_protocol::service::api::Health = serde_json::from_value(health).unwrap();
    runner.finish();
    result.unwrap();
    assert!(!path.exists());
    assert!(!receipt_path(&path).exists());
}
