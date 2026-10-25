//! Valid agent records, owned runtime services and temporary store fixtures.
use crate::runtime::{
    Core,
    core::CoreServices,
    domain::{Agent, AgentState, TaskOutcome},
    store::Store,
};
use crate::{
    config::{Config, Lifetime, Method, Profile, Provider, Sandbox},
    launch::LauncherCache,
    state::{Lock, State},
};
use std::{ops::Deref, path::Path, sync::Arc};

pub(crate) struct AgentBuilder(Agent);
impl AgentBuilder {
    pub fn new(id: &str) -> Self {
        Self(Agent {
            tool_catalog: hive_input::compatibility::Exact,
            id: id.into(),
            comb: None,
            mcp_servers: crate::config::McpServers::default(),
            role: Some("test".into()),
            name: None,
            parent_id: None,
            branched_from_agent_id: None,
            branch_hold: 0,
            branch_phase: None,
            last_finished_turn: None,
            last_attempted_turn: None,
            auth: "main".into(),
            auth_group: None,
            auth_decision: None,
            api_fallback: true,
            waiting_reason: None,
            quota_wait_until: None,
            auth_switch_pending: None,
            handoff: None,
            turn_auth_revision: None,
            execution_key: None,
            model_key: None,
            auth_probe_id: None,
            auth_checked_revision: 0,
            auth_generation: uuid::Uuid::nil(),
            recovery_fence: None,
            conversation_clean: false,
            profile: Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
            model: "mock".into(),
            effort: "low".into(),
            options_revision: 0,
            applied_options_revision: None,
            agent_options: crate::options::AgentOptions::default(),
            reserved_turn: None,
            session_selection: None,
            last_accepted_selection: None,
            turn_selection: None,
            redirect_pending: false,
            cwd: "/tmp".into(),
            instructions: String::new(),
            lifetime: Lifetime::Independent,
            sandbox: Sandbox::ReadOnly,
            state: AgentState::Starting,
            task_outcome: TaskOutcome::Open,
            paused: false,
            expired: false,
            thread_id: None,
            native_input_attempted: false,
            execution_id: None,
            turn_id: None,
            control: None,
            completion_pending: None,
            result: None,
            error: None,
            created_at: 0,
            ended_at: None,
            auth_cutoff: None,
            provider_process: None,
            steering: crate::runtime::domain::SteeringCapability::default(),
        })
    }
    pub fn configure(mut self, change: impl FnOnce(&mut Agent)) -> Self {
        change(&mut self.0);
        self
    }
    pub fn build(mut self) -> Agent {
        self.0.agent_options =
            crate::options::AgentOptions::new(self.0.desired_selection().options);
        self.0
    }
}

pub(crate) struct CoreBuilder(CoreServices);
impl CoreBuilder {
    pub fn new(path: &Path) -> Self {
        let state = State::new(path.join("state")).unwrap();
        let initialization = state.initialize().unwrap();
        drop(initialization);
        Self::from_store(
            path,
            Store::open(&path.join("runtime.sqlite")).unwrap(),
            state,
            super::daemon_lock(&path.join("state")),
            crate::launch::test_cache().unwrap(),
        )
    }
    pub fn from_store(
        path: &Path,
        store: Store,
        state: State,
        daemon_lock: Arc<Lock>,
        launcher: Arc<LauncherCache>,
    ) -> Self {
        Self(CoreServices {
            binary_sha256: "0".repeat(64),
            verification: None,
            store,
            config: Config {
                deprecations: Vec::new(),
                loaded: None,
                comb_collectors: std::collections::BTreeMap::default(),
                auth_groups: std::collections::BTreeMap::new(),
                providers: std::collections::BTreeMap::new(),
                default_provider: None,
                auth_profiles: std::collections::BTreeMap::new(),
                roles: std::collections::BTreeMap::new(),
                context: vec![],
                comb: None,
                mcp_servers: crate::config::McpServers::default(),
                runtime: crate::config::Runtime::default(),
                logging: crate::config::Logging::default(),
            },
            config_dir: path.into(),
            state,
            binaries: crate::runtime::Binaries::default().resolved(),
            daemon_lock,
            launcher,
            shutdown: std::sync::Arc::default(),
        })
    }
    pub fn build(self) -> Core {
        Core::new(self.0)
    }
}

pub(crate) struct TestStore {
    store: Store,
    pub temp: tempfile::TempDir,
}
impl TestStore {
    pub fn new() -> Self {
        let temp = crate::runtime::test_directory();
        let store = Store::open(&temp.path().join("db")).unwrap();
        Self { store, temp }
    }
    pub fn path(&self) -> std::path::PathBuf {
        self.temp.path().join("db")
    }
}
impl Deref for TestStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}
