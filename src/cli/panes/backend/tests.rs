use super::*;

const AGENT: &str = "11111111-2222-4333-8444-555555555555";
const OTHER: &str = "66666666-7777-4888-9999-aaaaaaaaaaaa";

fn command() -> PaneCommand {
    PaneCommand {
        agent: AGENT.to_owned(),
        program: PathBuf::from("/opt/my tools/\"hive\"\\bin/hive"),
        args: [
            "--state-dir",
            "/var/state dir",
            "attach",
            AGENT,
            "--exit-on-end",
        ]
        .map(String::from)
        .to_vec(),
    }
}

#[test]
fn pane_arguments_keep_the_program_and_its_arguments_as_separate_entries() {
    let argv = new_pane_argv(7, &command(), true);
    assert_eq!(
        argv,
        [
            "new-pane",
            "--tab-id",
            "7",
            "--name",
            &format!("hive:{AGENT}"),
            "--close-on-exit",
            "--no-focus",
            "--stacked",
            "--",
            "/opt/my tools/\"hive\"\\bin/hive",
            "--state-dir",
            "/var/state dir",
            "attach",
            AGENT,
            "--exit-on-end",
        ]
    );
    assert!(!new_pane_argv(7, &command(), false).contains(&String::from("--stacked")));
    assert_eq!(list_panes_argv(), ["list-panes", "--json", "--all"]);
    let tab = new_tab_argv("reviewer 11111111", &command());
    assert_eq!(
        &tab[..5],
        [
            "new-tab",
            "--no-focus",
            "--name",
            "reviewer 11111111",
            "--layout-string"
        ]
    );
}

#[test]
fn layouts_escape_paths_with_spaces_quotes_and_backslashes() {
    assert_eq!(kdl("plain"), "\"plain\"");
    assert_eq!(kdl("a \"b\" c\\d"), r#""a \"b\" c\\d""#);
    assert_eq!(kdl("line\nnext\u{7}"), r#""line\nnext\u{7}""#);
    let layout = layout(&command());
    assert!(
        layout.contains(r#"pane command="/opt/my tools/\"hive\"\\bin/hive" name="hive:11111111-2222-4333-8444-555555555555" close_on_exit=true { args "--state-dir" "/var/state dir" "attach" "11111111-2222-4333-8444-555555555555" "--exit-on-end"; };"#),
        "{layout}"
    );
    assert!(
        layout
            .starts_with("layout { pane size=1 borderless=true { plugin location=\"tab-bar\"; };")
    );
}

#[test]
fn recorded_pane_lists_yield_hive_panes_by_name_or_command() {
    // Trimmed from zellij 0.45.1 `list-panes --json --all`.
    let json = format!(
        r#"[
        {{"id": 0, "is_plugin": true, "title": "tab-bar", "exited": false, "terminal_command": null, "tab_id": 0}},
        {{"id": 0, "is_plugin": false, "title": "Pane #1", "exited": false, "terminal_command": null, "tab_id": 0, "pane_command": "/bin/zsh"}},
        {{"id": 1, "is_plugin": false, "title": "hive:{AGENT}", "exited": false, "terminal_command": "/usr/local/bin/hivectl --state-dir /s attach {AGENT} --exit-on-end", "tab_id": 1}},
        {{"id": 2, "is_plugin": false, "title": "renamed by hand", "exited": false, "terminal_command": "/usr/local/bin/hivectl attach {OTHER} --exit-on-end", "tab_id": 1}},
        {{"id": 3, "is_plugin": false, "title": "hive:panes", "exited": false, "terminal_command": "/usr/local/bin/hivectl panes", "tab_id": 0}},
        {{"id": 4, "is_plugin": false, "title": "hive:not-a-uuid", "exited": false, "terminal_command": null, "tab_id": 2}},
        {{"id": 5, "is_plugin": false, "title": "hive:{OTHER}", "exited": true, "terminal_command": null, "tab_id": 3}}
    ]"#
    );
    assert_eq!(
        parse_panes(&json).unwrap(),
        [
            Pane {
                agent: AGENT.to_owned(),
                tab: 1
            },
            Pane {
                agent: OTHER.to_owned(),
                tab: 1
            },
        ]
    );
    assert!(parse_panes("not json").is_err());
}
