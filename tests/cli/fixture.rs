// Shared auth fixtures; each harness supplies its own CLI process.
struct Fixture {
    tmp: tempfile::TempDir,
    config: PathBuf,
    state: State,
    scenario: PathBuf,
    codex: PathBuf,
    claude: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("hl-auth-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = tmp.path();
        let shebang = support::python_shebang(root);
        let config = root.join("config.yaml");
        fs::write(
            &config,
            concat!(
                "auth_profiles:\n",
                "  codex-main: {provider: codex, method: subscription}\n",
                "  claude-main: {provider: claude, method: subscription}\n",
                "  api: {provider: codex, method: api_key, api_key_env: HIVE_CANARY}\n",
            ),
        )
        .unwrap();
        let scenario = root.join("scenario.json");
        fs::write(&scenario, "{}").unwrap();
        fs::write(
            root.join("mock_common.py"),
            include_str!("../fixtures/mock_common.py"),
        )
        .unwrap();
        support::mock_files(root);
        let script = include_str!("../fixtures/provider.py");
        let make = |name: &str| {
            let path = root.join(name);
            fs::write(
                &path,
                format!(
                    "{shebang}SCENARIO = {}\nPROVIDER = {}\n{}",
                    json!(scenario),
                    json!(name),
                    script
                ),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        };
        let codex = make("codex");
        let claude = make("claude");
        // One private launcher root for the fixture's CLI processes. Test builds
        // of the CLI (the in-crate auth tests) read it, so they reuse one image
        // instead of copying and assessing a new one each; the release binary
        // already reuses its image under the account's own launcher root.
        fs::create_dir(root.join("launch")).unwrap();
        fs::set_permissions(root.join("launch"), fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            state: State::new(root.join("state")).unwrap(),
            config,
            scenario,
            codex,
            claude,
            tmp,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = support::command(cli_binary());
        cmd.args([
            "--config",
            self.config.to_str().unwrap(),
            "--state-dir",
            self.state.root.to_str().unwrap(),
            "--codex-bin",
            self.codex.to_str().unwrap(),
            "--claude-bin",
            self.claude.to_str().unwrap(),
            "--probe-timeout",
            "5",
            "auth",
        ]);
        cmd.env("HIVE_TEST_LAUNCH_ROOT", self.tmp.path().join("launch"));
        cmd
    }
    fn pty_command(&self, login_args: &[&str]) -> Command {
        let args: Vec<_> = self
            .command()
            .get_args()
            .map(std::borrow::ToOwned::to_owned)
            .collect();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_login.py");
        let mut command = support::command(support::python());
        command
            .arg(script)
            .arg(cli_binary())
            .args(args)
            .args(login_args)
            .env("HIVE_TEST_LAUNCH_ROOT", self.tmp.path().join("launch"));
        command
    }
    fn run(&self, args: &[&str]) -> (Output, Value) {
        let output = if matches!(
            args[0],
            "list" | "prune" | "generations" | "history-export" | "groups"
        ) {
            let mut command = support::command(cli_binary());
            command.arg("--state-dir").arg(&self.state.root);
            if args[0] == "list" {
                command.arg("--config").arg(&self.config);
            }
            command.arg("auth").args(args).output().unwrap()
        } else {
            self.command().args(args).output().unwrap()
        };
        let value = serde_json::from_slice(&output.stdout).expect("JSON result");
        (output, value)
    }
    fn register(&self, provider: Provider) -> hive::state::Binding {
        self.state
            .register(
                if provider == Provider::Codex {
                    "codex-main"
                } else {
                    "claude-main"
                },
                &Profile {
                    provider,
                    method: Method::Subscription,
                    api_key_env: None,
                    api_key_file: None,
                },
            )
            .unwrap()
    }
    // Consume fixture data once its serialized scenario has been written.
    fn scenario(&self, value: Value) {
        support::publish_fixture(&self.scenario, &serde_json::to_vec(&value).unwrap());
    }
}

fn login_pty_args(f: &Fixture, login_args: &[&str], cancel: bool) -> Output {
    let mut command = f.pty_command(login_args);
    if cancel {
        command.env("HIVE_PTY_CANCEL", "1");
    }
    if fs::read_to_string(&f.scenario)
        .unwrap()
        .contains("login_raw_enter")
    {
        command.env("HIVE_PTY_RAW", "1");
    }
    command.output().unwrap()
}

fn terminal_report(output: &Output) -> Value {
    // Human diagnostics may include sanitized objects; the response starts a line.
    let start = output
        .stdout
        .iter()
        .enumerate()
        .position(|(i, b)| *b == b'{' && (i == 0 || output.stdout[i - 1] == b'\n'))
        .unwrap();
    // A PTY merges channels: a checked human diagnostic can follow the JSON.
    let mut values =
        serde_json::Deserializer::from_slice(&output.stdout[start..]).into_iter::<Value>();
    let value = values.next().unwrap().unwrap();
    let suffix = String::from_utf8_lossy(&output.stdout[start + values.byte_offset()..]);
    if !suffix.trim().is_empty() {
        let mut escaped = value["error"]["message"]
            .as_str()
            .unwrap()
            .chars()
            .flat_map(char::escape_default);
        let message: String = escaped.by_ref().take(200).collect();
        let truncated = if escaped.next().is_some() { "..." } else { "" };
        assert_eq!(
            suffix.trim(),
            format!(
                "Hive: {}: {message}{truncated}",
                value["error"]["code"].as_str().unwrap()
            )
        );
    }
    value
}
