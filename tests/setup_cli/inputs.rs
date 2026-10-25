use super::*;

const PRIVATE: &str = "PRIVATE_RECEIVED_VALUE";

fn input(f: &Fixture, command: &mut Command, field: &str) -> Value {
    let (out, value) = output(command);
    assert_eq!(out.status.code(), Some(2), "{value}");
    assert_eq!(value["error"]["field"], field);
    for key in ["code", "message", "expected"] {
        assert!(
            value["error"][key].as_str().is_some_and(|v| !v.is_empty()),
            "{value}"
        );
    }
    assert!(
        value["error"]["examples"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains(PRIVATE));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(PRIVATE));
    assert!(!f.config.exists());
    assert!(!f.state.exists());
    value
}

fn answers(path: &Path, content: &[u8], mode: u32) {
    atomic_fs::Publisher::new(
        (atomic_fs::Options::new())
            .with_mode(mode)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .write(path, content)
    .unwrap();
}

#[test]
fn setup_answers_reject_unsafe_files_and_private_syntax_before_effects() {
    let f = Fixture::new();
    let path = f.home.path().join("answers.yaml");
    input(&f, f.command().arg("--answers").arg(&path), "--answers");
    input(
        &f,
        f.command().arg("--answers").arg(f.home.path()),
        "--answers",
    );
    for text in [
        format!("{PRIVATE}: true\n"),
        format!("login: {PRIVATE}\n"),
        format!("codex-profile: [{PRIVATE}]\n"),
        format!("provider: &x {PRIVATE}\nlogin: *x\n"),
        format!("login: !{PRIVATE} skip\n"),
        format!("login: skip\nlogin: {PRIVATE}\n"),
        format!("[ {PRIVATE}\n"),
    ] {
        answers(&path, text.as_bytes(), 0o600);
        input(&f, f.command().arg("--answers").arg(&path), "--answers");
    }
    answers(&path, &vec![b'x'; 1024 * 1024 + 1], 0o600);
    input(&f, f.command().arg("--answers").arg(&path), "--answers");
    answers(&path, b"login: skip\n", 0o666);
    input(&f, f.command().arg("--answers").arg(&path), "--answers");
    let link = f.home.path().join("answers-link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    input(&f, f.command().arg("--answers").arg(&link), "--answers");
    let fifo = f.home.path().join("answers-pipe");
    let native = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: live NUL-terminated private fixture path.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    input(&f, f.command().arg("--answers").arg(&fifo), "--answers");
}

#[test]
fn setup_supplied_values_name_the_flag_or_answers_key_before_effects() {
    let f = Fixture::new();
    let path = f.home.path().join("answers.yaml");
    for (key, value) in [
        ("codex-profile", "PRIVATE_RECEIVED_VALUE/slash"),
        ("claude-profile", "PRIVATE_RECEIVED_VALUE/slash"),
        ("service-path", "/bin:PRIVATE_RECEIVED_VALUE"),
        ("codex-model", "PRIVATE_RECEIVED_VALUE space"),
        ("claude-effort", PRIVATE),
    ] {
        if matches!(key, "codex-profile" | "claude-profile" | "service-path") {
            input(
                &f,
                f.command().arg(format!("--{key}")).arg(value),
                &format!("--{key}"),
            );
        }
        answers(&path, format!("{key}: {value}\n").as_bytes(), 0o600);
        let result = input(&f, f.command().arg("--answers").arg(&path), "--answers");
        assert!(result["error"]["expected"].as_str().unwrap().contains(key));
    }
    for (text, key) in [
        ("provider: []\n", "provider"),
        ("provider: [codex, codex]\n", "provider"),
        (
            "provider: [codex]\ndefault-provider: claude\n",
            "default-provider",
        ),
        ("provider: [claude]\nlogin: device\n", "login"),
    ] {
        answers(&path, text.as_bytes(), 0o600);
        let result = input(&f, f.command().arg("--answers").arg(&path), "--answers");
        assert!(result["error"]["expected"].as_str().unwrap().contains(key));
    }
    for (args, field) in [
        (vec!["--provider", "codex,codex"], "--provider"),
        (
            vec!["--provider", "codex", "--default-provider", "claude"],
            "--default-provider",
        ),
        (vec!["--provider", "claude", "--login", "device"], "--login"),
    ] {
        input(&f, f.command().args(args), field);
    }
}

#[test]
fn service_install_rejects_explicit_bad_path_before_config_or_manager() {
    let f = Fixture::new();
    input(
        &f,
        f.product("control").args([
            "service",
            "install",
            "--service-path",
            "/bin:PRIVATE_RECEIVED_VALUE",
        ]),
        "--service-path",
    );
}

#[cfg(feature = "comb")]
#[test]
fn forwarded_setup_errors_are_typed_private_and_keep_answers_provenance() {
    let f = Fixture::new();
    let binary = f.home.path().join("comb-input");
    let path = f.home.path().join("answers.yaml");
    // Comb reports its own flag; Hive names the flag the caller supplied.
    for (child, field) in [
        ("--bridge-file", "--bridge-file"),
        ("--root", "--comb-root"),
    ] {
        let rejection = json!({"error": {"code":"invalid_value", "message":PRIVATE,
            "field":child, "expected":PRIVATE, "examples":[PRIVATE]}});
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = version ]; then\n  echo '{{\"product\":\"Comb\",\"compiled_features\":[]}}'\n  exit 0\nfi\nprintf '%s\\n' '{rejection}'\necho {PRIVATE} >&2\nexit 2\n"
        );
        answers(&binary, script.as_bytes(), 0o700);
        let command = || {
            let mut c = f.command();
            c.args([
                "--yes",
                "--provider",
                "codex",
                "--login",
                "skip",
                "--codex-model",
                "fixture",
                "--codex-effort",
                "low",
                "--no-services",
                "--comb-bin",
            ])
            .arg(&binary);
            c
        };
        input(&f, command().arg(field).arg(PRIVATE), field);
        answers(
            &path,
            format!("{}: {PRIVATE}\n", &field[2..]).as_bytes(),
            0o600,
        );
        let result = input(&f, command().arg("--answers").arg(&path), "--answers");
        assert!(
            result["error"]["expected"]
                .as_str()
                .unwrap()
                .contains(&field[2..])
        );
    }
}

#[cfg(feature = "comb")]
#[test]
fn malformed_success_from_setup_child_does_not_replay_captured_stderr() {
    let f = Fixture::new();
    let binary = f.home.path().join("bad-comb");
    answers(
        &binary,
        format!("#!/bin/sh\necho {PRIVATE}\necho {PRIVATE} >&2\nexit 0\n").as_bytes(),
        0o700,
    );
    let (out, value) = output(
        f.command()
            .args([
                "--yes",
                "--provider",
                "codex",
                "--login",
                "skip",
                "--codex-model",
                "fixture",
                "--codex-effort",
                "low",
                "--no-services",
                "--comb-bin",
            ])
            .arg(&binary),
    );
    assert_eq!(out.status.code(), Some(1), "{value}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(PRIVATE));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(PRIVATE));
    assert!(!f.config.exists());
    assert!(!f.state.exists());
}

#[cfg(feature = "comb")]
#[test]
fn stored_comb_path_rejections_stay_operational_while_explicit_paths_are_input() {
    let f = Fixture::new();
    let bridge = f.home.path().join("stored-bridge.json");
    let root = f.home.path().join("stored-root");
    let connection = json!({"format":1,"socket": root.join(".comb/server.sock"),
        "credential": {"comb_id":"00000000-0000-4000-8000-000000000001",
        "id":"00000000-0000-4000-8000-000000000002", "secret":PRIVATE}});
    answers(&bridge, connection.to_string().as_bytes(), 0o600);
    let config = json!({"auth_profiles":{"codex-main":{"provider":"codex", "method":"subscription"}},
        "default_provider":"codex", "providers":{"codex":{"auth":"codex-main", "model":"fixture", "effort":"low"}},
        "comb":{"credential_file":bridge}}).to_string();
    answers(&f.config, config.as_bytes(), 0o600);
    let binary = f.home.path().join("comb-input");
    let source = f.home.path().join("answers.yaml");
    for (child, flag) in [
        ("--root", "--comb-root"),
        ("--bridge-file", "--bridge-file"),
    ] {
        let rejection = json!({"error":{"code":"invalid_value", "message":PRIVATE,
            "field":child, "expected":PRIVATE, "examples":[PRIVATE]}});
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = version ]; then\n  echo '{{\"product\":\"Comb\",\"compiled_features\":[]}}'\n  exit 0\nfi\nprintf '%s\\n' '{rejection}'\necho {PRIVATE} >&2\nexit 2\n"
        );
        answers(&binary, script.as_bytes(), 0o700);
        for origin in ["stored", "flag", "answers"] {
            let mut command = f.command();
            command
                .args(["--yes", "--login", "skip", "--no-services", "--comb-bin"])
                .arg(&binary);
            if origin == "flag" {
                command.arg(flag).arg(f.home.path().join("explicit"));
            } else if origin == "answers" {
                answers(
                    &source,
                    format!(
                        "{}: {}\n",
                        &flag[2..],
                        f.home.path().join("explicit").display()
                    )
                    .as_bytes(),
                    0o600,
                );
                command.arg("--answers").arg(&source);
            }
            let (out, value) = output(&mut command);
            if origin == "stored" {
                assert_eq!(out.status.code(), Some(1), "{value}");
                assert!(value["error"].get("field").is_none(), "{value}");
                assert_eq!(value["error"]["message"], "Setup child command failed.");
            } else {
                assert_eq!(out.status.code(), Some(2), "{value}");
                assert_eq!(
                    value["error"]["field"],
                    if origin == "flag" { flag } else { "--answers" }
                );
            }
            assert!(!String::from_utf8_lossy(&out.stdout).contains(PRIVATE));
            assert!(!String::from_utf8_lossy(&out.stderr).contains(PRIVATE));
            assert_eq!(fs::read_to_string(&f.config).unwrap(), config);
            assert!(!f.state.exists());
        }
    }
}
