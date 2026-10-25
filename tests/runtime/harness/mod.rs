#![allow(dead_code)] // Shared fixture subsets exercise distinct native capabilities.
use super::*;
mod builder;
mod views;

pub(super) struct Harness {
    pub(super) tmp: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) config: PathBuf,
    pub(super) binary: PathBuf,
    pub(super) provider_home: PathBuf,
    pub(super) daemon: Option<support::ChildGuard>,
}
impl Harness {
    pub(super) fn new() -> Self {
        Self::builder(Provider::Codex).build()
    }
    pub(super) fn builder(provider: Provider) -> builder::HarnessBuilder {
        builder::HarnessBuilder::new(provider)
    }
    pub(super) fn start(&mut self) {
        self.start_binary(cli_binary(), false);
    }
    pub(super) fn start_binary(&mut self, binary: &std::path::Path, debug_timings: bool) {
        let mut command = support::command(binary);
        if debug_timings {
            command.env("HIVE_LOG", "debug");
        }
        self.start_command(command);
    }
    pub(super) fn start_with_env(&mut self, name: &str, value: &str) {
        let mut command = support::command(cli_binary());
        command.env(name, value);
        self.start_command(command);
    }
    fn start_command(&mut self, mut command: std::process::Command) {
        self.daemon = Some(support::ChildGuard::new(
            command
                .arg("--config")
                .arg(&self.config)
                .arg("--state-dir")
                .arg(&self.root)
                .arg("--codex-bin")
                .arg(&self.binary)
                .arg("--claude-bin")
                .arg(self.tmp.path().join("claude"))
                .args(["daemon", "run"])
                .stdout(fs::File::create(self.tmp.path().join("daemon.stdout.log")).unwrap())
                .stderr(fs::File::create(self.tmp.path().join("daemon.stderr.log")).unwrap())
                .spawn()
                .unwrap(),
        ));
        let mut last_ping = String::new();
        let ready = support::wait::try_wait_until(
            support::wait::STARTUP_TIMEOUT,
            support::wait::POLL_INTERVAL,
            || {
                let ping = runtime::client(&self.root, "ping", json!({"op":"ping"}));
                last_ping = format!("{ping:?}");
                if ping.is_ok() {
                    return Some(true);
                }
                self.daemon
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .map(|_| false)
            },
        );
        assert!(
            ready == Some(true),
            "daemon startup failed; ping={last_ping}; stdout={}; stderr={}; stack={}",
            fs::read_to_string(self.tmp.path().join("daemon.stdout.log")).unwrap_or_default(),
            fs::read_to_string(self.tmp.path().join("daemon.stderr.log")).unwrap_or_default(),
            self.startup_failure_trace(ready.is_none())
        );
    }
    #[cfg_attr(
        not(target_os = "macos"),
        expect(
            clippy::unused_self,
            reason = "Only the macOS stack sampler needs the harness paths and child identity."
        )
    )]
    fn startup_failure_trace(&self, alive: bool) -> String {
        if !alive {
            return String::new(); // try_wait reaped the child; its PID could be reused.
        }
        #[cfg(target_os = "macos")]
        {
            let path = self.tmp.path().join("daemon.startup.sample");
            // Diagnostic only, after the unchanged readiness deadline has failed.
            let sample = support::command("/usr/bin/sample")
                .arg(self.daemon.as_ref().unwrap().id().to_string())
                .args(["1", "10", "-file"])
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(child) = sample {
                let mut child = support::ChildGuard::new(child);
                let _ = support::wait::try_wait_until(
                    Duration::from_secs(3),
                    support::wait::POLL_INTERVAL,
                    || match child.try_wait() {
                        Ok(None) => None,
                        result => Some(result),
                    },
                );
            }
            fs::read_to_string(path).unwrap_or_default()
        }
        #[cfg(not(target_os = "macos"))]
        String::new()
    }
    pub(super) fn call_id(&self, id: &str, args: Value) -> Value {
        runtime::client(&self.root, id, args).unwrap_or_else(|error| {
            let db = rusqlite::Connection::open(self.root.join("runtime/runtime.sqlite")).unwrap();
            let mut query = db
                .prepare("SELECT data FROM events WHERE kind = 'provider_shutdown'")
                .unwrap();
            let reports: Vec<String> = query
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            panic!("{error:?}; provider shutdowns: {reports:?}");
        })
    }
    pub(super) fn call(&self, args: Value) -> Value {
        self.call_id(&Uuid::new_v4().to_string(), args)
    }
    pub(super) fn ok(&self, args: Value) -> Value {
        let response = self.call(args);
        assert!(
            response.get("error").is_none_or(Value::is_null),
            "{response}"
        );
        response
    }
    pub(super) fn expect_err(&self, args: Value, code: &str) -> Value {
        let response = self.call(args);
        assert_eq!(response["error"]["code"], code, "{response}");
        response
    }
    pub(super) fn resume_after_wakeup(&self, id: &str) {
        // Sending to an unpaused stopped agent may start it before this request arrives.
        let response = self.call(json!({"op":"resume","target":id}));
        let requested = json!({"agent_id":id,"operation":"resume","status":"requested"});
        let already_starting = json!({
            "error": {
                "code": "agent_busy",
                "message": "Wait for the current transition before resuming."
            }
        });
        assert!(
            response == requested || response == already_starting,
            "{response}"
        );
    }
    pub(super) fn provider_home(&self, provider: Provider) -> PathBuf {
        let state = State::new(self.root.clone()).unwrap();
        let profile = Profile {
            provider,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        };
        let name = if provider == Provider::Codex {
            "main"
        } else {
            "claude"
        };
        let binding = state.lookup(name, &profile).unwrap().unwrap();
        binding.directory.join("provider")
    }
    pub(super) fn set_scenario(&self, provider: Provider, scenario: Value) {
        Self::write_scenario(&self.provider_home(provider), scenario);
    }
    pub(super) fn write_scenario(home: &std::path::Path, scenario: Value) {
        support::publish_fixture(
            &home.join("runtime-scenario.json"),
            &serde_json::to_vec(&scenario).unwrap(),
        );
    }
    pub(super) fn update_scenario(&self, provider: Provider, change: impl FnOnce(&mut Value)) {
        let path = self.provider_home(provider).join("runtime-scenario.json");
        let mut scenario = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut scenario);
        support::publish_fixture(&path, &serde_json::to_vec(&scenario).unwrap());
    }
    pub(super) fn edit_config(&self, change: impl FnOnce(&mut Value)) {
        let mut config: Value =
            serde_yaml_ng::from_slice(&fs::read(&self.config).unwrap()).unwrap();
        change(&mut config);
        support::publish_fixture(&self.config, &serde_json::to_vec(&config).unwrap());
    }
    pub(super) fn spawn(&self, body: &str) -> String {
        let v =
            self.ok(json!({"op":"spawn","role":"implementor","cwd":self.tmp.path(),"body":body}));
        v["agent_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{v}"))
            .into()
    }
    pub(super) fn wait(&self, id: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        self.wait_timeout(id, support::wait::TEST_TIMEOUT, predicate)
    }
    /// The agent's status once it is idle with nothing left to start. `state ==
    /// "idle"` alone also matches before a spawned agent's first turn: while its
    /// first message waits to be claimed (`queued` is 1) and while the claimed
    /// turn starts (`idle_since` is null).
    ///
    /// The queue is read before the status: a message claimed by then shows its
    /// turn in the later status read. In the other order, a status read before
    /// the claim and a queue read after it would pass before the turn ran.
    pub(super) fn settled(&self, id: &str) -> Value {
        let mut last = Value::Null;
        support::wait::try_wait_until(
            support::wait::TEST_TIMEOUT,
            Duration::from_millis(30),
            || {
                if self.ok(json!({"op":"tree","root":id}))["agents"][0]["queued"] != 0 {
                    return None;
                }
                last = self.status(id);
                (last["state"] == "idle"
                    && last["turn_id"].is_null()
                    && !last["idle_since"].is_null())
                .then(|| last.clone())
            },
        )
        .unwrap_or_else(|| panic!("agent {id} did not settle: {last}"))
    }
    pub(super) fn wait_timeout(
        &self,
        id: &str,
        timeout: Duration,
        predicate: impl Fn(&Value) -> bool,
    ) -> Value {
        let mut last = Value::Null;
        support::wait::try_wait_until(timeout, Duration::from_millis(30), || {
            last = self.status(id);
            predicate(&last).then(|| last.clone())
        })
        .unwrap_or_else(|| panic!("agent {id} did not reach expected state: {last}"))
    }
    pub(super) fn shutdown(&mut self) {
        let response = self.ok(json!({"op":"shutdown"}));
        assert_eq!(response["status"], "stopping", "{response}");
        let child = self.daemon.as_mut().unwrap();
        let status = support::wait::wait_until(
            support::wait::SHUTDOWN_TIMEOUT,
            support::wait::POLL_INTERVAL,
            "successful daemon shutdown",
            || child.try_wait().unwrap(),
        );
        assert!(
            status.success(),
            "daemon exited unsuccessfully: {status}; stdout={}; stderr={}; log tail={}",
            fs::read_to_string(self.tmp.path().join("daemon.stdout.log")).unwrap_or_default(),
            fs::read_to_string(self.tmp.path().join("daemon.stderr.log")).unwrap_or_default(),
            self.log_tail(),
        );
        self.daemon = None;
    }
    /// The daemon's last diagnostic records, kept for a failure message before
    /// the temporary state is removed.
    fn log_tail(&self) -> String {
        let log = fs::read_to_string(self.root.join("runtime/logs/hive-daemon.jsonl"))
            .unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    }
    pub(super) fn reports(&self) -> Vec<Value> {
        let db = rusqlite::Connection::open(self.root.join("runtime/runtime.sqlite")).unwrap();
        let mut query = db
            .prepare("SELECT data FROM events WHERE kind = 'provider_shutdown' ORDER BY seq")
            .unwrap();
        query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }
    pub(super) fn assert_recovery_required(&self) {
        let state = State::new(self.root.clone()).unwrap();
        let profile = Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        };
        let binding = state.lookup("main", &profile).unwrap().unwrap();
        let report = serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap();
        assert!(
            report["executions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["conversation_safe"] == false)
        );
        assert_eq!(
            serde_json::to_value(state.recovery_report(&binding).unwrap()).unwrap()["runtime_usable"],
            true
        );
        state.runtime_lease(&binding).unwrap();
        assert!(!self.root.join("runtime/owner.json").exists());
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = runtime::client(&self.root, "shutdown", json!({"op":"shutdown"}));
            let _ = support::wait::try_wait_until(
                support::wait::STARTUP_TIMEOUT,
                support::wait::POLL_INTERVAL,
                || child.try_wait().ok().flatten(),
            );
            // ChildGuard kills and reaps the owned child if graceful exit failed.
        }
    }
}
