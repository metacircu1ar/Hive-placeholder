use super::*;
use std::path::PathBuf;

const AGENT: &str = "11111111-2222-4333-8444-555555555555";
const OTHER: &str = "66666666-7777-4888-9999-aaaaaaaaaaaa";

fn terminal(tab: &str, id: &str, name: &str) -> Terminal {
    Terminal {
        window: String::from("w1"),
        tab: tab.to_owned(),
        id: id.to_owned(),
        name: name.to_owned(),
    }
}

#[test]
fn listings_parse_separated_rows_and_adopt_only_exact_agent_titles() {
    let text = format!(
        "w1\u{1f}t1\u{1f}a\u{1f}hive:panes:1a2b3c4d\u{1e}\
         w1\u{1f}t2\u{1f}b\u{1f}hive:{AGENT}\u{1e}\
         w1\u{1f}t2\u{1f}c\u{1f}vim: hive:{OTHER} | notes\u{1e}\n"
    );
    let rows = parse_listing(&text);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[1],
        Terminal {
            window: String::from("w1"),
            tab: String::from("t2"),
            id: String::from("b"),
            name: format!("hive:{AGENT}"),
        }
    );
    let agents: Vec<_> = rows.iter().filter_map(Terminal::agent).collect();
    assert_eq!(agents, [AGENT]);
    assert!(parse_listing("").is_empty());
}

#[test]
fn splits_fill_a_two_by_two_grid_from_the_tabs_agent_terminals() {
    let agent = |n: u8| format!("hive:{}", AGENT.replacen('1', &n.to_string(), 1));
    let mut listing = vec![
        terminal("t1", "shell", "zsh"),
        terminal("t1", "a", &agent(1)),
    ];
    assert_eq!(
        split_target(&listing, "t1"),
        Some((String::from("a"), "right"))
    );
    listing.push(terminal("t1", "b", &agent(2)));
    assert_eq!(
        split_target(&listing, "t1"),
        Some((String::from("a"), "down"))
    );
    listing.push(terminal("t1", "c", &agent(3)));
    assert_eq!(
        split_target(&listing, "t1"),
        Some((String::from("b"), "down"))
    );
    assert_eq!(split_target(&listing, "t2"), None);
}

#[test]
fn commands_survive_the_shell_with_spaces_quotes_and_backslashes() {
    let pane = PaneCommand {
        agent: AGENT.to_owned(),
        program: PathBuf::from("/opt/my tools/it's \"hive\"\\bin"),
        args: ["--state-dir", "/var/a b", "attach", AGENT, "$HOME `x` ;|&"]
            .map(String::from)
            .to_vec(),
    };
    let command = shell_command(&pane);
    let script = format!("set -- {command}; for word; do printf '[%s]' \"$word\"; done");
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .output()
        .unwrap();
    let expected = std::iter::once(pane.program.to_string_lossy().into_owned())
        .chain(pane.args.iter().cloned())
        .fold(String::new(), |mut expected, word| {
            expected.push('[');
            expected.push_str(&word);
            expected.push(']');
            expected
        });
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[test]
fn error_codes_decide_between_ending_and_retrying() {
    let denied = "execution error: Not authorized to send Apple events to Ghostty. (-1743)";
    assert_eq!(error_code(denied), Some(-1743));
    assert_eq!(error_code("no code here"), None);
    for code in [-1743, -1744, -1708, -2741, -2753] {
        assert!(
            matches!(classify(Some(code), ""), Failure::Fatal(error) if error.code() == "pane_backend_unavailable")
        );
    }
    for code in [Some(-1728), Some(-10000), None] {
        assert!(
            matches!(classify(code, "gone (-1728)"), Failure::Retry(text) if text.contains("Ghostty script failed"))
        );
    }
}

#[test]
fn tab_ids_map_to_stable_keys() {
    let mut ghostty = Ghostty::new(String::from("/dev/ttys001"));
    let first = ghostty.key("tab-a");
    let second = ghostty.key("tab-b");
    assert_ne!(first, second);
    assert_eq!(ghostty.key("tab-a"), first);
    assert_eq!(ghostty.ids.get(&second).map(String::as_str), Some("tab-b"));
}

#[test]
fn script_arguments_keep_their_order() {
    let pane = PaneCommand {
        agent: AGENT.to_owned(),
        program: PathBuf::from("/bin/hivectl"),
        args: vec![String::from("attach"), AGENT.to_owned()],
    };
    let command = shell_command(&pane);
    assert_eq!(
        new_tab_args("w1", &pane, "worker 11111111"),
        [
            String::from("w1"),
            command.clone(),
            String::from("worker 11111111")
        ]
    );
    assert_eq!(
        split_args("t9", "down", &pane),
        [String::from("t9"), String::from("down"), command]
    );
}

#[test]
fn splits_created_in_one_pass_feed_the_next_split_target() {
    // Simulate the listing updates open_pane makes after each split.
    let agent = |n: u8| format!("hive:{}", AGENT.replacen('1', &n.to_string(), 1));
    let mut listing = vec![terminal("t1", "a", &agent(1))];
    let mut targets = Vec::new();
    for (n, id) in [(2, "b"), (3, "c"), (4, "d")] {
        let (target, direction) = split_target(&listing, "t1").unwrap();
        targets.push((target, direction));
        listing.push(terminal("t1", id, &agent(n)));
    }
    assert_eq!(
        targets,
        [
            (String::from("a"), "right"),
            (String::from("a"), "down"),
            (String::from("b"), "down")
        ]
    );
}
