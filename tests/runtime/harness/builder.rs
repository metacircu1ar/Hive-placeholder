use super::*;

pub(crate) struct HarnessBuilder {
    provider: Provider,
    config: Value,
    scenario: Value,
}
impl HarnessBuilder {
    pub(crate) fn new(provider: Provider) -> Self {
        let mut config = json!({"auth_profiles":{"main":{"provider":"codex","method":"subscription"}},"roles":{}});
        for (role, model, lifetime, sandbox) in [
            (
                "implementor",
                "gpt-5.6-terra",
                "independent",
                "workspace-write",
            ),
            ("reviewer", "gpt-5.6-sol", "with_parent", "read-only"),
            ("pong", "gpt-5.6-sol", "independent", "read-only"),
            ("ping", "gpt-5.6-terra", "independent", "read-only"),
        ] {
            config["roles"][role] = json!({"auth":"main","model":model,"instructions":"role.md","lifetime":lifetime,"sandbox":sandbox});
        }
        config["roles"]["implementor"]
            .as_object_mut()
            .unwrap()
            .remove("sandbox");
        if provider == Provider::Claude {
            config["auth_profiles"]["claude"] =
                json!({"provider":"claude","method":"subscription"});
            for (role, lifetime) in [
                ("claude", "independent"),
                ("claude-reviewer", "with_parent"),
            ] {
                config["roles"][role] = json!({"auth":"claude","model":"sonnet","effort":"high","instructions":"role.md","lifetime":lifetime});
            }
        }
        Self {
            provider,
            config,
            scenario: json!({}),
        }
    }
    pub(crate) fn runtime(mut self, settings: Value) -> Self {
        self.config["runtime"] = settings;
        self
    }
    pub(crate) fn auth_group(mut self, name: &str, members: &[&str]) -> Self {
        self.config["auth_groups"][name] = json!({"mode":"ordered","members":members});
        for member in members {
            if self.config["auth_profiles"].get(member).is_none() {
                self.config["auth_profiles"][member] =
                    json!({"provider":self.provider,"method":"subscription"});
            }
        }
        self
    }
    pub(crate) fn scenario(mut self, settings: Value) -> Self {
        self.scenario = settings;
        self
    }
    pub(crate) fn build(self) -> Harness {
        let tmp = tempfile::Builder::new()
            .prefix("hl-")
            .tempdir_in("/tmp")
            .unwrap();
        let base = tmp.path();
        let root = base.join("state");
        let config = base.join("config.json");
        fs::write(base.join("role.md"), "Test role").unwrap();
        fs::write(&config, serde_json::to_vec(&self.config).unwrap()).unwrap();
        let state = State::new(root.clone()).unwrap();
        for (name, profile) in self.config["auth_profiles"].as_object().unwrap() {
            let profile: Profile = serde_json::from_value(profile.clone()).unwrap();
            state.register(name, &profile).unwrap();
        }
        let profile = Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        };
        let binding = state.lookup("main", &profile).unwrap().unwrap();
        let provider_home = binding.directory.join("provider");
        let shebang = support::python_shebang(base);
        fs::write(
            base.join("mock_common.py"),
            include_str!("../../fixtures/mock_common.py"),
        )
        .unwrap();
        support::mock_files(base);
        let binary = base.join("codex");
        Self::executable(
            &binary,
            &shebang,
            include_str!("../../fixtures/runtime_provider.py"),
        );
        if self.provider == Provider::Claude {
            Self::executable(
                &base.join("claude"),
                &shebang,
                include_str!("../../fixtures/claude_provider.py"),
            );
        }
        let fixture = Harness {
            tmp,
            root,
            config,
            binary,
            provider_home,
            daemon: None,
        };
        fixture.set_scenario(self.provider, self.scenario);
        fixture
    }
    fn executable(path: &std::path::Path, shebang: &str, source: &str) {
        fs::write(path, format!("{shebang}{source}")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}
