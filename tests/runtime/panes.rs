//! `hivectl panes` refusals, driven through a terminal with a stand-in `zellij`,
//! and panes in a real, private tmux server.
use super::*;
use support::wait::{POLL_INTERVAL, TEST_TIMEOUT, wait_until};

/// A `zellij` that reports a version and fails every action, so the controller
/// runs without a real session.
fn fake_zellij(f: &Harness) -> std::path::PathBuf {
    let bin = f.tmp.path().join("fake-zellij");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("zellij");
    fs::write(
        &script,
        "#!/bin/sh\necho \"$@\" >> \"$ZELLIJ_FAKE_LOG\"\n[ \"$1\" = --version ] && echo 'zellij 0.45.1' && exit 0\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin
}

/// Run `hivectl panes` under a terminal with these steps and environment.
fn controller(
    f: &Harness,
    args: &[&str],
    env: &[(&str, &str)],
    steps: &Value,
) -> std::process::Child {
    let mut command = support::command(support::python());
    command
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_attach.py"))
        .arg(cli_binary())
        .arg("--state-dir")
        .arg(&f.root)
        .arg("panes")
        .args(args)
        .env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID")
        // A suite run inside tmux must never drive the developer's session.
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        // Never select Ghostty here: scripting it raises a macOS Automation prompt.
        .env_remove("TERM_PROGRAM")
        .env("HIVE_PTY_STEPS", steps.to_string())
        .stdout(std::process::Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    command.spawn().unwrap()
}

/// Wait until a controller has run a pass: it lists panes only after taking
/// the session lock, so a later controller is then refused.
fn wait_for_first_pass(log: &std::path::Path) {
    wait_until(
        Duration::from_secs(10),
        Duration::from_millis(50),
        "first pass",
        || {
            fs::read_to_string(log)
                .ok()
                .filter(|calls| calls.contains("list-panes"))
                .map(drop)
        },
    );
}

fn finished(child: std::process::Child) -> Value {
    let output = child.wait_with_output().unwrap();
    let result: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)));
    assert_eq!(
        result["failed_step"],
        Value::Null,
        "{}",
        result["screen"].as_str().unwrap()
    );
    result
}

#[test]
fn panes_refuse_outside_zellij_a_second_controller_and_an_unknown_root() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let bin = fake_zellij(&f);
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let log = f.tmp.path().join("fake-zellij.log");
    let log_text = log.to_str().unwrap().to_owned();
    let inside: [(&str, &str); 4] = [
        ("ZELLIJ", "0"),
        ("ZELLIJ_SESSION_NAME", "hive-test"),
        ("PATH", &path),
        ("ZELLIJ_FAKE_LOG", &log_text),
    ];
    // Outside a zellij session.
    let outside = finished(controller(
        &f,
        &[],
        &[],
        &json!([["wait", "pane_backend_unavailable"]]),
    ));
    assert_eq!(outside["status"], 1, "{outside}");
    let no_tmux = finished(controller(
        &f,
        &["--backend", "tmux"],
        &[],
        &json!([["wait", "runs inside a tmux session"]]),
    ));
    assert_eq!(no_tmux["status"], 1, "{no_tmux}");
    // A running controller holds the session; a second one is refused.
    let first = controller(
        &f,
        &[],
        &inside,
        &json!([["wait", "Hive panes:"], ["sleep", 6], ["send", "q"]]),
    );
    wait_for_first_pass(&log);
    let second = finished(controller(
        &f,
        &[],
        &inside,
        &json!([["wait", "pane_controller_running"]]),
    ));
    assert_eq!(second["status"], 1, "{second}");
    let first = finished(first);
    assert_eq!(first["status"], 0, "{first}");
    // The zellij failures show in the status list instead of ending the controller.
    assert!(
        first["screen"]
            .as_str()
            .unwrap()
            .contains("zellij list-panes failed"),
        "{first}"
    );
    // An unknown root is refused with the daemon's error.
    let unknown = finished(controller(
        &f,
        &["--root", "no-such-agent"],
        &inside,
        &json!([["wait", "unknown_agent"]]),
    ));
    assert_eq!(unknown["status"], 1, "{unknown}");
    assert!(
        unknown["screen"]
            .as_str()
            .unwrap()
            .contains("cannot follow the agent tree")
    );
    // Without a zellij executable the controller refuses before taking the lock.
    let missing = [
        ("ZELLIJ", "0"),
        ("ZELLIJ_SESSION_NAME", "hive-test"),
        ("PATH", "/usr/bin:/bin"),
    ];
    let missing = finished(controller(
        &f,
        &[],
        &missing,
        &json!([["wait", "zellij was not found"]]),
    ));
    assert_eq!(missing["status"], 1, "{missing}");
    f.shutdown();
}

/// The variables a multiplexer or terminal passes on; a private tmux server
/// copies its client's environment, so none of them may leak into it.
const OUTER: [&str; 6] = [
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "TERM_PROGRAM",
    "TMUX",
    "TMUX_PANE",
];

/// A private tmux server: its own socket, no user configuration and a holder
/// pane, so it outlives the controller. Dropping it ends the server.
struct TmuxServer {
    socket: std::path::PathBuf,
}

impl TmuxServer {
    /// `None` when tmux cannot run here.
    fn start(dir: &std::path::Path) -> Option<Self> {
        let version = support::command("tmux").arg("-V").output();
        if !version.is_ok_and(|output| output.status.success()) {
            return None;
        }
        let server = Self {
            socket: dir.join("tmux.sock"),
        };
        server.run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "hive",
            "-x",
            "200",
            "-y",
            "60",
            "--",
            "/bin/sh",
            "-c",
            "while :; do sleep 1; done",
        ]);
        Some(server)
    }

    fn command(&self) -> std::process::Command {
        let mut command = support::command("tmux");
        // Without TMUX or a UTF-8 locale, tmux prints the formats' tabs as `_`.
        command.arg("-u").arg("-S").arg(&self.socket);
        for name in OUTER {
            command.env_remove(name);
        }
        command
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Every pane: (window, pane, start command).
    fn panes(&self) -> Vec<(String, String, String)> {
        self.run(&[
            "list-panes",
            "-s",
            "-t",
            "hive",
            "-F",
            "#{window_id}\t#{pane_id}\t#{pane_start_command}",
        ])
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some((
                fields.next()?.to_owned(),
                fields.next()?.to_owned(),
                fields.next()?.to_owned(),
            ))
        })
        .collect()
    }

    /// The window of the agent's view, if one is open.
    fn view(&self, agent: &str) -> Option<String> {
        let marker = format!("attach {agent}");
        self.panes()
            .into_iter()
            .find(|(_, _, command)| command.contains(&marker))
            .map(|(window, _, _)| window)
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        // The server may already be gone; there is nothing else to clean.
        let _ = self.command().arg("kill-server").output();
    }
}

#[test]
fn a_manual_view_clears_its_title_before_returning_to_a_shell() {
    let mut f = Harness::builder(Provider::Codex).build();
    let Some(tmux) = TmuxServer::start(f.tmp.path()) else {
        assert!(
            std::env::var("HIVE_TEST_REQUIRE_TMUX").as_deref() != Ok("1"),
            "tmux is required but cannot run"
        );
        eprintln!("skipped: tmux cannot run on this host");
        return;
    };
    f.start();
    let agent = f.spawn("hold");
    f.wait(&agent, |a| a["state"] == "running");
    // The shell survives attach, as it does when someone opens a view by hand.
    let pane = tmux.run(&[
        "new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "hive:", "--",
        "/bin/sh", "-c",
        "\"$0\" --state-dir \"$1\" attach \"$2\" --exit-on-end --title; printf '\\nattach-exited\\n'; exec /bin/sh",
        cli_binary().to_str().unwrap(),
        f.root.to_str().unwrap(),
        &agent,
    ]);
    let pane = pane.trim();
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "manual view title", || {
        (tmux
            .run(&["display-message", "-p", "-t", pane, "#{pane_title}"])
            .trim()
            == format!("hive:{agent}"))
        .then_some(())
    });
    tmux.run(&["send-keys", "-t", pane, "/quit", "Enter"]);
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "return to shell", || {
        tmux.run(&["capture-pane", "-p", "-t", pane])
            .contains("attach-exited")
            .then_some(())
    });
    assert_eq!(
        tmux.run(&[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{pane_dead}:#{pane_title}"
        ])
        .trim(),
        "0:",
        "the live shell must no longer identify itself as an agent view"
    );
    assert_eq!(f.status(&agent)["state"], "running");
    f.shutdown();
}

#[test]
fn panes_open_close_and_group_agents_in_a_real_tmux_session() {
    let mut f = Harness::builder(Provider::Codex).build();
    let Some(tmux) = TmuxServer::start(f.tmp.path()) else {
        assert!(
            std::env::var("HIVE_TEST_REQUIRE_TMUX").as_deref() != Ok("1"),
            "tmux is required but cannot run"
        );
        eprintln!("skipped: tmux cannot run on this host");
        return;
    };
    f.start();
    let parent = f.spawn("spawn bound child");
    let child = wait_until(TEST_TIMEOUT, POLL_INTERVAL, "bound child", || {
        let all = f.ok(json!({"op": "agents", "parent": parent}));
        all["agents"][0]["id"].as_str().map(str::to_owned)
    });
    f.wait(&child, |a| a["state"] == "running");
    let root = f.root.canonicalize().unwrap();
    let controller = tmux.run(&[
        "new-window",
        "-d",
        "-P",
        "-F",
        "#{window_id}\t#{pane_id}",
        "-t",
        "hive:",
        "-n",
        "controller",
        "--",
        cli_binary().to_str().unwrap(),
        "--state-dir",
        root.to_str().unwrap(),
        "panes",
        "--linger",
        "1",
        "--no-mouse",
    ]);
    let (controller_window, controller_pane) = controller.trim().split_once('\t').unwrap();
    // The parent gets its own window and its child splits it.
    let window = wait_until(TEST_TIMEOUT, POLL_INTERVAL, "agent views", || {
        let parent_window = tmux.view(&parent)?;
        (tmux.view(&child)? == parent_window).then_some(parent_window)
    });
    assert_ne!(window, controller_window);
    let in_window: Vec<_> = tmux
        .panes()
        .into_iter()
        .filter(|(w, _, _)| *w == window)
        .collect();
    assert_eq!(in_window.len(), 2);
    // --no-mouse reaches every view the controller opens.
    assert!(
        in_window
            .iter()
            .all(|(_, _, command)| command.contains("--no-mouse")),
        "{in_window:?}"
    );
    // New windows and panes never take focus from where the user is.
    let current = tmux.run(&["display-message", "-p", "-t", "hive", "#{window_index}"]);
    assert_eq!(current.trim(), "0");
    other_controllers(&f, &tmux, &root);
    // Leaving the controller closes its window and leaves the views open.
    tmux.run(&["send-keys", "-t", controller_pane, "q"]);
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "controller exit", || {
        (!tmux.panes().iter().any(|(w, _, _)| w == controller_window)).then_some(())
    });
    assert_eq!(tmux.view(&parent), Some(window.clone()));
    // The views close themselves once their agents end.
    f.ok(json!({"op": "terminate", "target": parent}));
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "views closed", || {
        (tmux.view(&parent).is_none() && tmux.view(&child).is_none()).then_some(())
    });
    assert_eq!(tmux.panes().len(), 1, "only the holder remains");
    f.shutdown();
}

/// A controller in its own terminal, pointed at the private tmux server with
/// the holder pane as its own.
fn outside_controller(
    tmux: &TmuxServer,
    root: &std::path::Path,
    env: &[(&str, std::ffi::OsString)],
    steps: &Value,
) -> std::process::Child {
    let holder = tmux.run(&["display-message", "-p", "-t", "hive:0", "#{pane_id}"]);
    let server = tmux.run(&["display-message", "-p", "#{socket_path},#{pid},0"]);
    let mut command = support::command(support::python());
    command
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_attach.py"))
        .arg(cli_binary())
        .arg("--state-dir")
        .arg(root)
        .arg("panes");
    for name in OUTER {
        command.env_remove(name);
    }
    command
        .env("TMUX", server.trim())
        .env("TMUX_PANE", holder.trim())
        .envs(env.iter().map(|(name, value)| (*name, value)))
        .env("HIVE_PTY_STEPS", steps.to_string())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

/// With a controller running: a second one for the session is refused, and
/// nested multiplexers are told apart by who owns the terminal.
fn other_controllers(f: &Harness, tmux: &TmuxServer, root: &std::path::Path) {
    let second = outside_controller(
        tmux,
        root,
        &[],
        &json!([["wait", "pane_controller_running"]]),
    );
    assert_eq!(finished(second)["status"], 1);
    // With zellij's variables also set, a controller in a tmux pane still
    // chooses tmux, so the session lock refuses it… The variables come from
    // env (new-window -e needs tmux 3.2), and the paths reach sh as its
    // arguments, never as shell text.
    let nested = tmux.run(&[
        "new-window",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-t",
        "hive:",
        "--",
        "/usr/bin/env",
        "ZELLIJ=0",
        "ZELLIJ_SESSION_NAME=nested",
        "/bin/sh",
        "-c",
        "\"$0\" --state-dir \"$1\" panes; echo exit=$?; sleep 60",
        cli_binary().to_str().unwrap(),
        root.to_str().unwrap(),
    ]);
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "nested refusal", || {
        let screen = tmux.run(&["capture-pane", "-p", "-t", nested.trim()]);
        (screen.contains("pane_controller_running") && screen.contains("exit=1")).then_some(())
    });
    tmux.run(&["kill-pane", "-t", nested.trim()]);
    // …while the same variables outside that pane's terminal mean zellij. The
    // stand-in zellij comes first; tmux stays reachable for the check.
    let path = format!(
        "{}:{}",
        fake_zellij(f).display(),
        std::env::var("PATH").unwrap()
    );
    let env = [
        ("PATH", path.into()),
        (
            "ZELLIJ_FAKE_LOG",
            f.tmp.path().join("fake-zellij.log").into(),
        ),
        ("ZELLIJ", "0".into()),
        ("ZELLIJ_SESSION_NAME", "nested".into()),
    ];
    let steps = json!([["wait", "zellij list-panes failed"], ["send", "q"]]);
    assert_eq!(
        finished(outside_controller(tmux, root, &env, &steps))["status"],
        0
    );
}
