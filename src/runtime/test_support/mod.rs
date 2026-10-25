//! Runtime test builders, owned fixture resources and injected one-shot hooks.
mod builders;
mod fixture;
mod writer;
pub(super) use builders::{AgentBuilder, CoreBuilder, TestStore};
pub(super) use fixture::RuntimeFixture;
pub(super) use writer::WriterGate;
pub(crate) fn test_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("private runtime test directory")
}

/// Hold the same ownership file as a daemon for every in-process runtime fixture.
pub(super) fn daemon_lock(root: &std::path::Path) -> std::sync::Arc<crate::state::Lock> {
    crate::state::directory(root).expect("private fixture state directory");
    let runtime = root.join("runtime");
    crate::state::directory(&runtime).expect("private runtime ownership directory");
    std::sync::Arc::new(
        crate::state::Lock::acquire(&runtime.join("daemon.lock"))
            .expect("exclusive runtime fixture ownership"),
    )
}

/// One-shot hooks injected by tests without changing production storage layout.
type SqlHook = Box<dyn FnMut(&rusqlite::Connection) -> rusqlite::Result<()> + Send>;
#[derive(Default)]
pub(crate) struct TestHooks {
    sql: std::cell::RefCell<Option<(&'static str, SqlHook)>>,
    checkpoint: std::cell::RefCell<Option<Box<dyn FnOnce() + Send>>>,
    deferred_checkpoint: std::cell::Cell<bool>,
    output: std::cell::RefCell<Option<Box<dyn FnOnce() + Send>>>,
    before_job: std::cell::RefCell<Option<Box<dyn FnOnce() + Send>>>,
}
impl TestHooks {
    pub(super) fn at_sql(
        &self,
        phase: &'static str,
        hook: impl FnMut(&rusqlite::Connection) -> rusqlite::Result<()> + Send + 'static,
    ) {
        *self.sql.borrow_mut() = Some((phase, Box::new(hook)));
    }
    pub(super) fn sql(&self, phase: &str, db: &rusqlite::Connection) -> rusqlite::Result<()> {
        if let Some((expected, hook)) = &mut *self.sql.borrow_mut()
            && *expected == phase
        {
            hook(db)?;
        }
        Ok(())
    }

    pub(super) fn before_checkpoint(&self, hook: impl FnOnce() + Send + 'static) {
        *self.checkpoint.borrow_mut() = Some(Box::new(hook));
    }
    pub(super) fn checkpoint_pending(&self) -> bool {
        self.checkpoint.borrow().is_some()
    }
    /// Make the next committed batch run a deferred checkpoint, as at the WAL threshold.
    pub(super) fn request_deferred_checkpoint(&self) {
        self.deferred_checkpoint.set(true);
    }
    pub(super) fn deferred_checkpoint_pending(&self) -> bool {
        self.deferred_checkpoint.replace(false)
    }
    pub(super) fn checkpoint(&self) {
        if let Some(hook) = self.checkpoint.borrow_mut().take() {
            hook();
        }
    }
    pub(super) fn before_output(&self, hook: impl FnOnce() + Send + 'static) {
        *self.output.borrow_mut() = Some(Box::new(hook));
    }
    pub(super) fn output(&self) {
        if let Some(hook) = self.output.borrow_mut().take() {
            hook();
        }
    }
    pub(super) fn before_job(hook: impl FnOnce() + Send + 'static) -> Self {
        Self {
            before_job: std::cell::RefCell::new(Some(Box::new(hook))),
            ..Self::default()
        }
    }
    pub(super) fn job(&self) {
        if let Some(hook) = self.before_job.borrow_mut().take() {
            hook();
        }
    }
}

pub(super) fn agent() -> super::domain::Agent {
    AgentBuilder::new("agent")
        .configure(|agent| {
            agent.state = super::domain::AgentState::Running;
            agent.model = "gpt-5.6-luna".into();
            agent.instructions = "corpus".into();
            agent.provider_process = Some(super::domain::ProviderProcess::Reserved {
                id: "generation".into(),
            });
            agent.thread_id = Some("thread".into());
            agent.execution_id = Some("execution".into());
            agent.turn_id = Some("turn".into());
        })
        .build()
}

/// Capture real cross-thread diagnostics in an isolated test process.
pub(super) fn failure_log(module: &str, test: &str, action: impl FnOnce()) -> Option<String> {
    const ISOLATED: &str = "HIVE_FAILURE_LOG_TEST";
    let (_, module) = module.split_once("::").unwrap();
    let name = format!("{module}::{test}");
    if std::env::var(ISOLATED).ok().as_deref() != Some(&name) {
        let mut command = crate::test_support::rerun_isolated(&name, false);
        command.env(ISOLATED, &name);
        crate::test_support::run_isolated(&mut command);
        return None;
    }
    let directory = test_directory();
    let path = directory.path().join("failure.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.install_global().unwrap();
    action();
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    Some(std::fs::read_to_string(path).unwrap())
}

/// A pinned metadata probe followed by the scenario's ordinary provider behavior.
pub(super) fn codex_binary(binary: &std::path::Path, behavior: &str) {
    use std::os::unix::fs::PermissionsExt;
    let python = crate::process::executable(std::ffi::OsStr::new("python3")).unwrap();
    let schema =
        serde_json::to_string(include_str!("../../../tests/fixtures/codex-schema.json")).unwrap();
    std::fs::write(
        binary,
        format!(
            "#!{}\nSCHEMA = {schema}\n{}\n{behavior}",
            python.display(),
            "\nimport \
        json, pathlib, sys\nif '--version' in sys.argv:\n    print('codex-cli 0.161.0')\n    \
        sys.exit(0)\nif 'generate-json-schema' in sys.argv:\n    \
        pathlib.Path(sys.argv[sys.argv.index('--out') + \
        1]).joinpath('ClientRequest.json').write_text(SCHEMA)\n    sys.exit(0)\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o700)).unwrap();
}

pub(super) fn failing_catalog(fixture: &RuntimeFixture) -> std::path::PathBuf {
    let binary = fixture.temp.path().join("mock-catalog");
    codex_binary(
        &binary,
        "\nfor line in sys.stdin:\n    request = json.loads(line)\n    if 'id' not in \
            request:\n        continue\n    if request['method'] == 'initialize':\n        \
            result = {}\n    else:\n        # Corrupt this probe's private receipt before the \
            failure response, making\n        # finish fail before it can mark the owned \
            execution confirmed.\n        for receipt in \
            pathlib.Path.cwd().parent.joinpath('executions').glob('*.json'):\n            \
            receipt.write_text('secret corrupt receipt')\n        print(json.dumps({'id': \
            request['id'], 'error': {'code': -1, 'message': 'secret native discovery'}}), \
            flush=True)\n        continue\n    print(json.dumps({'id': request['id'], \
            'result': result}), flush=True)\n",
    );
    binary
}
