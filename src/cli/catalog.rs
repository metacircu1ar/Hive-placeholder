//! Command examples shared by help and local input errors.
use hive_input::Examples;

pub(super) const COMMANDS: &[(&str, &[&str])] = &[
    ("config", &["hivectl config check"]),
    (
        "config check",
        &["hivectl config check", "hivectl config check --fix"],
    ),
    ("", &["hivectl --help"]),
    (
        "setup",
        &[
            "hivectl setup",
            "hivectl setup --yes --answers answers.yaml --dry-run",
        ],
    ),
    ("service", &["hivectl service status"]),
    ("service install", &["hivectl service install"]),
    ("service start", &["hivectl service start"]),
    ("service stop", &["hivectl service stop"]),
    ("service status", &["hivectl service status"]),
    ("service uninstall", &["hivectl service uninstall"]),
    ("doctor", &["hivectl doctor"]),
    ("version", &["hivectl version --json"]),
    (
        "logs",
        &[
            "hivectl logs --tail 100",
            "hivectl logs --level warn --follow",
        ],
    ),
    ("approvals", &["hivectl approvals --wait --timeout 60"]),
    (
        "approvals show",
        &["hivectl approvals show 00000000-0000-4000-8000-000000000001"],
    ),
    (
        "respond",
        &["hivectl respond 00000000-0000-4000-8000-000000000001 --accept --request-id answer-1"],
    ),
    ("mcp", &["hivectl mcp"]),
    ("auth", &["hivectl auth list"]),
    ("state", &["hivectl state status"]),
    ("state status", &["hivectl state status"]),
    (
        "state upgrade",
        &["hivectl state upgrade --dry-run", "hivectl state upgrade"],
    ),
    ("auth groups", &["hivectl auth groups"]),
    (
        "auth usage",
        &["hivectl auth usage", "hivectl auth usage codex-main"],
    ),
    ("auth list", &["hivectl auth list"]),
    ("auth status", &["hivectl auth status codex-main"]),
    ("auth login", &["hivectl auth login codex-main --browser"]),
    ("auth recover", &["hivectl auth recover codex-main"]),
    ("auth generations", &["hivectl auth generations codex-main"]),
    (
        "auth history-export",
        &["hivectl auth history-export --output /private/controller-history"],
    ),
    (
        "auth prune",
        &["hivectl auth prune codex-main --generation 00000000-0000-4000-8000-000000000001"],
    ),
    (
        "auth repair",
        &["hivectl auth repair codex-main --reauthenticate --browser"],
    ),
    (
        "compact",
        &[
            "hivectl compact --before 1700000000 --keep-per-agent 100",
            "hivectl compact --before 1700000000 --keep-per-agent 100 --dry-run",
        ],
    ),
    (
        "output",
        &["hivectl output 00000000-0000-4000-8000-000000000001 --offset 0 --limit 65536"],
    ),
    ("models", &["hivectl models --auth codex-main"]),
    ("options", &["hivectl options catalog"]),
    (
        "options catalog",
        &["hivectl options catalog --provider codex --model gpt-6-luna"],
    ),
    (
        "verify-steering",
        &["hivectl verify-steering --auth claude-main --model sonnet --effort low"],
    ),
    ("daemon", &["hivectl daemon status"]),
    (
        "daemon run",
        &["hivectl --config examples/auth.yaml daemon run"],
    ),
    (
        "daemon start",
        &[
            "hivectl --config examples/auth.yaml daemon start",
            "hivectl daemon stop",
        ],
    ),
    (
        "daemon restart",
        &["hivectl --config examples/auth.yaml daemon restart"],
    ),
    ("daemon stop", &["hivectl daemon stop"]),
    ("daemon status", &["hivectl daemon status"]),
    (
        "spawn",
        &["hivectl spawn reviewer --cwd /absolute/repo --body-file review.md"],
    ),
    ("agents", &["hivectl agents --limit 100"]),
    (
        "batch",
        &["hivectl batch --operations-file batch.json --join --request-id batch-1"],
    ),
    ("status", &["hivectl status AGENT_ID"]),
    (
        "read",
        &[
            "hivectl read AGENT_ID --after 0 --limit 100",
            "hivectl read AGENT_ID --follow",
            "hivectl read join:JOIN_ID",
        ],
    ),
    (
        "live",
        &["hivectl live AGENT_ID", "hivectl live AGENT_ID --follow"],
    ),
    (
        "attach",
        &[
            "hivectl attach AGENT_ID",
            "hivectl attach AGENT_ID --read-only",
            "hivectl attach AGENT_ID --exit-on-end --linger 10 --title",
        ],
    ),
    (
        "panes",
        &[
            "hivectl panes",
            "hivectl panes --root AGENT_ID --max-panes 8 --linger 10",
        ],
    ),
    (
        "tui",
        &[
            "hivectl tui",
            "hivectl tui --root AGENT_ID --max-views 4 --keep-ended --quiet",
        ],
    ),
    ("messages", &["hivectl messages AGENT_ID --limit 100"]),
    (
        "send",
        &[
            "hivectl send AGENT_ID --body Review --request-id follow-up-1",
            "hivectl send AGENT_ID --body-file follow-up.md --request-id follow-up-2",
        ],
    ),
    (
        "set-auth",
        &["hivectl set-auth AGENT_ID codex-backup --request-id login-change-1"],
    ),
    (
        "set-name",
        &[
            "hivectl set-name AGENT_ID reviewer --request-id rename-1",
            "hivectl set-name AGENT_ID --clear --request-id rename-2",
        ],
    ),
    (
        "set-options",
        &[
            "hivectl set-options AGENT_ID --option model=gpt-6-luna --option effort=low \
                --request-id settings-1",
        ],
    ),
    (
        "complete",
        &[
            "hivectl complete AGENT_ID --body Done --request-id finished-1",
            "hivectl complete AGENT_ID --result-file result.md --request-id finished-2",
        ],
    ),
    #[cfg(feature = "comb")]
    (
        "evidence",
        &["hivectl evidence correction --summary-file correction.md --request-id correction-1"],
    ),
    #[cfg(feature = "comb")]
    ("collectors", &["hivectl collectors status"]),
    #[cfg(feature = "comb")]
    ("collectors status", &["hivectl collectors status"]),
    #[cfg(feature = "comb")]
    (
        "collectors discard",
        &["hivectl collectors discard project"],
    ),
    #[cfg(feature = "comb")]
    (
        "collectors release-uncertain",
        &["hivectl collectors release-uncertain project ITEM_ID"],
    ),
    #[cfg(feature = "comb")]
    (
        "collectors rebind",
        &["hivectl collectors rebind project 00000000-0000-4000-8000-000000000001"],
    ),
    ("stop", &["hivectl stop AGENT_ID --request-id stop-1"]),
    (
        "terminate",
        &["hivectl terminate AGENT_ID --request-id terminate-1"],
    ),
    ("purge", &["hivectl purge AGENT_ID --dry-run"]),
    ("gc", &["hivectl gc --older-than 30d --dry-run"]),
    ("resume", &["hivectl resume AGENT_ID --request-id resume-1"]),
    (
        "interrupt",
        &["hivectl interrupt AGENT_ID --request-id interrupt-1"],
    ),
    (
        "wait",
        &[
            "hivectl wait AGENT_ID --until completed --timeout 60",
            "hivectl wait --join JOIN_ID --timeout 45",
        ],
    ),
    ("hive-daemon", &["hive-daemon --config examples/auth.yaml"]),
];

#[expect(
    clippy::expect_used,
    reason = "Static example metadata is validated for every entry by the command coverage test"
)]
pub(super) fn examples(path: &str) -> Option<Examples> {
    COMMANDS
        .iter()
        .find(|(key, _)| *key == path)
        .map(|(_, examples)| {
            Examples::new(examples.iter().map(|s| (*s).to_owned()).collect())
                .expect("valid command examples")
        })
}

pub(super) fn decorate(command: clap::Command, path: &str) -> clap::Command {
    let command = match examples(path) {
        Some(examples) => command.after_help(examples.help()),
        None => command,
    };
    command.mut_subcommands(|child| {
        let path = if path.is_empty() {
            child.get_name().to_owned()
        } else {
            format!("{path} {}", child.get_name())
        };
        decorate(child, &path)
    })
}
