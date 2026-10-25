//! Real daemon shutdown/recovery around a test-only SQLite commit fault.
use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use serde_json::{Value, json};
use std::sync::OnceLock;

use crate::integration_support as support;

static FAULT: OnceLock<PathBuf> = OnceLock::new();
const ROOT: &str = "HIVE_WRITER_FAILURE_TEST_ROOT";
const FAULT_ENV: &str = "HIVE_WRITER_FAILURE_TEST_ARMED";
const TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(20);

pub(super) fn install_fault(store: &Store) {
    if let Some(marker) = FAULT.get() {
        let marker = marker.clone();
        let mut fired = false;
        store.test_hooks().at_sql("commit", move |_| {
            if !fired && marker.exists() {
                fired = true;
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
                    Some("sk-secret-sql INSERT INTO sensitive VALUES(secret)".into()),
                ));
            }
            Ok(())
        });
    }
}

#[test]
#[ignore = "isolated daemon subprocess"]
fn daemon_fixture() -> Result<()> {
    let base = PathBuf::from(std::env::var_os(ROOT).unwrap());
    if std::env::var_os(FAULT_ENV).is_some() {
        FAULT.set(base.join("fault")).unwrap();
    }
    serve(
        base.join("config.json"),
        base.join("state"),
        super::super::Binaries {
            codex: base.join("codex").into_os_string(),
            claude: base.join("unused-claude").into_os_string(),
        },
    )
}

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    child: Option<ChildGuard>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("lx-writer-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let base = temp.path();
        let config = json!({
            "auth_profiles":{"main":{"provider":"codex","method":"subscription"}},
            "roles":{"test":{"auth":"main","model":"gpt-5.6-luna","effort":"medium","instructions":"role.md"}},
            "runtime":{"sessions_per_codex_process":1,"max_providers":4,"max_loaded":4,"max_active_turns":4,"max_starting":4}
        });
        fs::write(
            base.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        fs::write(base.join("role.md"), "Fixture").unwrap();
        let state = State::new(base.join("state")).unwrap();
        let profile = serde_json::from_value(config["auth_profiles"]["main"].clone()).unwrap();
        let home = state
            .register("main", &profile)
            .unwrap()
            .directory
            .join("provider");
        fs::write(home.join("runtime-scenario.json"), b"{}").unwrap();
        support::mock_files(base);
        fs::write(
            base.join("mock_common.py"),
            include_str!("../../../tests/fixtures/mock_common.py"),
        )
        .unwrap();
        fs::write(
            base.join("codex"),
            format!(
                "{}{}",
                support::python_shebang(base),
                include_str!("../../../tests/fixtures/runtime_provider.py")
            ),
        )
        .unwrap();
        fs::set_permissions(base.join("codex"), fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            temp,
            home,
            child: None,
        }
    }
    fn root(&self) -> PathBuf {
        self.temp.path().join("state")
    }
    fn start(&mut self, fault: bool) {
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut command = rerun_isolated(&format!("{module}::daemon_fixture"), true);
        command
            .env(ROOT, self.temp.path())
            .stdout(fs::File::create(self.temp.path().join("stdout")).unwrap())
            .stderr(fs::File::create(self.temp.path().join("stderr")).unwrap());
        if fault {
            command.env(FAULT_ENV, "1");
        }
        self.child = Some(ChildGuard(crate::os::spawn(&mut command).unwrap()));
        let root = self.root();
        wait_until(TIMEOUT, POLL, || {
            if super::super::client(&root, "ping", json!({"op":"ping"})).is_ok() {
                return Some(());
            }
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "daemon startup failed: {}",
                fs::read_to_string(self.temp.path().join("stderr")).unwrap()
            );
            None
        });
    }
    fn call(&self, args: Value) -> Value {
        let value =
            super::super::client(&self.root(), &uuid::Uuid::new_v4().to_string(), args).unwrap();
        assert!(value["error"].is_null(), "{value}");
        value
    }
    fn exit(&mut self) -> std::process::ExitStatus {
        let status = wait_until(TIMEOUT, POLL, || {
            self.child.as_mut().unwrap().try_wait().unwrap()
        });
        self.child.take();
        status
    }
    fn spawn(&self, body: &str) -> String {
        self.call(json!({"op":"spawn","role":"test","cwd":self.temp.path(),"body":body}))["agent_id"].as_str().unwrap().into()
    }
    fn agent(&self, id: &str) -> Value {
        self.call(json!({"op":"status","target":id}))["agent"].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = super::super::client(&self.root(), "fixture-stop", json!({"op":"shutdown"}));
            let until = std::time::Instant::now() + TIMEOUT;
            while self.child.as_mut().unwrap().try_wait().unwrap().is_none()
                && std::time::Instant::now() < until
            {
                std::thread::sleep(POLL);
            }
            // ChildGuard reaps on failure before TempDir removes evidence.
            self.child.take();
        }
    }
}

#[test]
fn writer_failure_shuts_down_active_providers_and_restart_never_replays_input() {
    let mut f = Fixture::new();
    f.start(true);
    let ids: Vec<_> = (0..3).map(|_| f.spawn("hold")).collect();
    let before: Vec<_> = ids
        .iter()
        .map(|id| {
            wait_until(TIMEOUT, POLL, || {
                let agent = f.agent(id);
                (agent["state"] == "running" && agent["turn_id"].is_string()).then_some(agent)
            })
        })
        .collect();
    let identities: Vec<_> = before
        .iter()
        .map(|agent| {
            state::ProcessIdentity::read(
                u32::try_from(agent["provider_pid"].as_u64().unwrap()).unwrap(),
            )
            .unwrap()
            .unwrap()
        })
        .collect();
    let original_inputs = wait_until(TIMEOUT, POLL, || {
        let bytes = fs::read_to_string(f.home.join("inputs.jsonl")).ok()?;
        (bytes.lines().count() == 3).then_some(bytes)
    });
    fs::write(f.temp.path().join("fault"), "arm").unwrap();
    let _ = super::super::client(
        &f.root(),
        "fault-trigger",
        json!({"op":"send","target":ids[0],"body":"sk-secret-message-body"}),
    );
    assert!(!f.exit().success());
    assert!(
        f.root().join("runtime/owner.json").exists(),
        "uncertain writer cleanup must retain ownership evidence"
    );
    for identity in identities {
        assert!(
            identity.absent().unwrap(),
            "mock provider survived shutdown"
        );
    }
    assert_primary_diagnostic(&f);
    f.start(false);
    for (id, previous) in ids.iter().zip(&before) {
        let now = f.agent(id);
        assert_eq!(now["state"], "stopped");
        assert_eq!(now["thread_id"], previous["thread_id"]);
        assert!(now["turn_id"].is_null());
    }
    let db = rusqlite::Connection::open(f.root().join("runtime/runtime.sqlite")).unwrap();
    for id in &ids {
        let states: Vec<String> = db
            .prepare("SELECT state FROM messages WHERE recipient=?1 ORDER BY seq")
            .unwrap()
            .query_map([id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(states, ["uncertain"]);
    }
    drop(db);
    let fresh = f.spawn("complete");
    wait_until(TIMEOUT, POLL, || {
        (f.agent(&fresh)["task_outcome"] == "completed").then_some(())
    });
    let inputs = fs::read_to_string(f.home.join("inputs.jsonl")).unwrap();
    assert!(inputs.starts_with(&original_inputs));
    assert_eq!(inputs.lines().count(), 4, "uncertain input was replayed");
    f.call(json!({"op":"shutdown"}));
    assert!(f.exit().success());
}

fn assert_primary_diagnostic(f: &Fixture) {
    let log = fs::read_to_string(f.root().join("runtime/logs/hive-daemon.jsonl")).unwrap();
    assert!(!log.contains("sk-secret"));
    assert!(!log.contains("INSERT INTO sensitive"));
    let rows: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        rows.iter().any(|r| r["event"] == "writer.failed"
            && r["sqlite_code"] == rusqlite::ffi::SQLITE_IOERR_WRITE)
    );
    assert!(
        rows.iter().any(|r| r["event"] == "daemon.failed"
            && r["code"] == "database_error"
            && r["sqlite_code"] == rusqlite::ffi::SQLITE_IOERR_WRITE),
        "{rows:?}"
    );
}
