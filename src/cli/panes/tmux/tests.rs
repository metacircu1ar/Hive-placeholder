use super::*;
use std::path::PathBuf;

const AGENT: &str = "11111111-2222-4333-8444-555555555555";
const ROOT: &str = "/var/state dir";

fn view(agent: &str) -> PaneCommand {
    PaneCommand {
        agent: agent.to_owned(),
        program: PathBuf::from("/opt/my tools/hive"),
        args: [
            "--state-dir",
            ROOT,
            "attach",
            agent,
            "--exit-on-end",
            "--linger",
            "3",
        ]
        .map(String::from)
        .to_vec(),
    }
}

#[test]
fn commands_use_the_argument_form_and_escape_trailing_semicolons() {
    let mut pane = view(AGENT);
    pane.args.push(String::from("ends;"));
    pane.args.push(String::from(";"));
    assert_eq!(
        new_window_argv("$3", "lead 11111111", &pane),
        [
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{window_id}",
            "-t",
            "$3:",
            "-n",
            "lead 11111111",
            "--",
            "/opt/my tools/hive",
            "--state-dir",
            ROOT,
            "attach",
            AGENT,
            "--exit-on-end",
            "--linger",
            "3",
            "ends\\;",
            "\\;",
        ]
    );
    assert_eq!(
        split_argv(12, &view(AGENT))[..10],
        [
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            "@12",
            "--",
            "/opt/my tools/hive",
            "--state-dir"
        ]
    );
    assert_eq!(layout_argv(12), ["select-layout", "-t", "@12", "tiled"]);
    assert_eq!(
        remain_argv(12),
        ["set-option", "-w", "-t", "@12", "remain-on-exit", "off"]
    );
    assert_eq!(kill_argv("%7"), ["kill-pane", "-t", "%7"]);
    assert_eq!(
        announce_argv("%0"),
        ["select-pane", "-t", "%0", "-T", "hive:panes"]
    );
    assert_eq!(
        list_argv("$3"),
        [
            "list-panes",
            "-s",
            "-t",
            "$3",
            "-F",
            "#{window_id}\t#{pane_id}\t#{pane_dead}\t#{pane_title}\t#{pane_start_command}"
        ]
    );
    assert_eq!(session_argv("%0")[4], "#{session_id}\t#{socket_path}");
    assert_eq!(tty_argv("%0")[4], "#{pane_tty}");
    // tmux turns `\;` back into `;`, including an escaped backslash before it.
    assert_eq!(escape("a\\;"), "a\\\\;");
    assert_eq!(escape("plain"), "plain");
}

#[test]
fn words_read_tmuxs_start_command_quoting() {
    // tmux 3.6b's rendering of the argv
    // /bin/sh -c 'echo "$@" > /tmp/x/out; sleep 60' zero 'a"b' 'c\d' 'e$f' 'g h' "it's" 'p\;' '#{pane_id}' 'x~y'
    let shown = r##"/bin/sh -c "echo \"$@\" > /tmp/x/out; sleep 60" zero 'a"b' c\\d "e\$f" "g h" "it's" "p;" "#{pane_id}" x~y"##;
    assert_eq!(
        words(shown),
        [
            "/bin/sh",
            "-c",
            r#"echo "$@" > /tmp/x/out; sleep 60"#,
            "zero",
            "a\"b",
            "c\\d",
            "e$f",
            "g h",
            "it's",
            "p;",
            "#{pane_id}",
            "x~y",
        ]
    );
    assert!(words("").is_empty());
    assert_eq!(words("''"), [""]);
    // tmux 3.6b escapes C-style inside single quotes and outside quotes too:
    // the argv 'a"\b' "it's \"x\"" 'tab<TAB>in' ü is shown as below.
    assert_eq!(
        words(r#"'a"\\b' "it's \"x\"" tab\tin ü \101"#),
        ["a\"\\b", "it's \"x\"", "tab\tin", "ü", "A"]
    );
}

#[test]
fn listings_adopt_agent_views_and_mark_this_controllers_own() {
    // tmux 3.6b shows the program `/opt/it's "x"/hive` as "/opt/it's \"x\"/hive".
    let own = format!(
        r#""/opt/it's \"x\"/hive" --state-dir "{ROOT}" attach {AGENT} --exit-on-end --linger 3"#
    );
    let other_root =
        format!("/usr/bin/hivectl --state-dir /elsewhere attach {AGENT} --exit-on-end");
    let kept = format!("/usr/bin/hivectl --state-dir \"{ROOT}\" attach {AGENT}");
    let shell_form = format!("\"hivectl attach {AGENT}\"");
    let text = format!(
        "@0\t%0\t0\tshell\t/bin/zsh\n\
         @3\t%4\t0\thive:{AGENT}\t{own}\n\
         @3\t%5\t1\thive:{AGENT}\t{own}\n\
         @4\t%6\t1\thive:{AGENT}\t{other_root}\n\
         @4\t%7\t1\tshell\t{kept}\n\
         @5\t%8\t0\thive:{AGENT}\t{shell_form}\n\
         @6\t%9\t0\thive:{AGENT}\thive attach not-a-uuid\n\
         @7\t%10\t0\tshell\techo\twith\ttabs attach {AGENT}\n\
         broken line\n\
         @x\t%11\t0\tsh\n\
         @8\t%12\t2\tsh\n"
    );
    let listed = parse_listing(&text, ROOT);
    let summary: Vec<(u64, &str, bool, Option<&str>, bool)> = listed
        .iter()
        .map(|pane| {
            (
                pane.window,
                pane.pane.as_str(),
                pane.dead,
                pane.agent.as_deref(),
                pane.owned,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (0, "%0", false, None, false),
            (3, "%4", false, Some(AGENT), true),
            (3, "%5", true, Some(AGENT), true),
            (4, "%6", true, Some(AGENT), false),
            (4, "%7", true, Some(AGENT), false),
            (5, "%8", false, Some(AGENT), false),
            (6, "%9", false, Some(AGENT), false),
            (7, "%10", false, Some(AGENT), false),
        ]
    );
    assert!(parse_listing("", ROOT).is_empty());
    assert_eq!(window_key("@12"), Some(12));
    assert_eq!(window_key("12"), None);
}

#[test]
fn navigation_changes_identity_without_changing_cleanup_ownership() {
    let next = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let command = format!("hivectl --state-dir '{ROOT}' attach {AGENT} --exit-on-end --title");
    for dead in [0, 1] {
        let listed = parse_listing(&format!("@1\t%2\t{dead}\thive:{next}\t{command}"), ROOT);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].agent.as_deref(), Some(next));
        assert!(listed[0].owned);
    }
    assert_eq!(
        Tmux::new("$1".into(), "%0".into(), ROOT.into()).attach_flags(),
        &["--title"]
    );
}

#[test]
fn views_are_identified_before_startup_and_without_the_title_flag() {
    let command = format!("hivectl --state-dir '{ROOT}' attach {AGENT}");
    for title in ["", "shell", "hive:not-a-uuid"] {
        let listed = parse_listing(&format!("@1\t%2\t0\t{title}\t{command}"), ROOT);
        assert_eq!(listed[0].agent.as_deref(), Some(AGENT));
        assert!(!listed[0].owned);
    }
    // A valid title identifies the current agent independently of the launch text.
    let listed = parse_listing(&format!("@1\t%2\t0\thive:{AGENT}\t/bin/sh"), ROOT);
    assert_eq!(listed[0].agent.as_deref(), Some(AGENT));
    assert!(!listed[0].owned);
}

#[test]
fn a_view_starting_after_the_verification_deadline_is_not_opened_twice() {
    use super::super::plan::{self, History, Limits, Pane, PaneState, Row};
    use std::time::{Duration, Instant};
    let rows = [Row {
        id: AGENT.into(),
        parent: None,
        role: None,
        name: None,
        state: "idle".into(),
    }];
    let limits = Limits {
        max_panes: 8,
        per_tab: PER_TAB,
        stacks: false,
    };
    let mut history = History::default();
    let now = Instant::now();
    let first = plan::reconcile(&rows, &[], &mut history, &limits, now);
    assert_eq!(first.actions.len(), 1);
    history.opened(&first.actions[0], Some(1), now);
    let command = format!("hivectl --state-dir '{ROOT}' attach {AGENT} --exit-on-end --title");
    let panes: Vec<Pane> = parse_listing(&format!("@1\t%2\t0\tshell\t{command}"), ROOT)
        .into_iter()
        .map(|p| Pane {
            agent: p.agent.unwrap(),
            tab: p.window,
        })
        .collect();
    for delay in [
        plan::VERIFY + Duration::from_millis(1),
        plan::VERIFY + plan::RETRIES[0] + Duration::from_secs(1),
    ] {
        let pass = plan::reconcile(&rows, &panes, &mut history, &limits, now + delay);
        assert!(
            pass.actions.is_empty(),
            "duplicate opened before a title arrived"
        );
        assert!(matches!(history.state(AGENT), Some(PaneState::Open { .. })));
    }
}
