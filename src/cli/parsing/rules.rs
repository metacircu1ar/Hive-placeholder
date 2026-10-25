use hive_input::clap::{FieldRule, Received};

const MODEL: &str = "a supported model token, 1..128 ASCII bytes";
const EFFORT: &str = "none, minimal, low, medium, high, xhigh, max or ultra";

/// Only these explicit schema fields admit received values. Everything else is
/// omitted, including bodies, credentials, paths, JSON cursors and instructions.
pub(super) fn for_command(command: &clap::Command, path: &str) -> Vec<FieldRule<'static>> {
    const GLOBALS: &[(&str, &str)] = &[
        ("probe_timeout", "an integer from 1 to 300 seconds"),
        ("login_timeout", "an integer from 1 to 3600 seconds"),
    ];
    let mut result: Vec<_> = GLOBALS
        .iter()
        .filter(|(id, _)| has(command, id))
        .map(|&(id, expected)| FieldRule {
            id,
            expected,
            received: Received::Public,
        })
        .collect();
    let fields = fields(path);
    result.extend(fields.iter().map(|&(id, expected)| FieldRule {
        id,
        expected,
        received: Received::Public,
    }));
    if path == "agents" {
        result.push(FieldRule {
            id: "stalled_after",
            expected: "a JSON object with idle_since (integer 0..9223372036854775807) and \
                root_id (1..128 UTF-8 bytes)",
            received: Received::Omit,
        });
    }
    result
}

fn fields(path: &str) -> &'static [(&'static str, &'static str)] {
    match path {
        "logs" => &[
            ("tail", "an integer from 1 to 2000 records"),
            ("level", "error, warn, info, debug or trace"),
        ],
        "spawn" => &[
            ("provider", "codex or claude"),
            ("model", MODEL),
            ("effort", EFFORT),
            ("sandbox", "read-only, workspace-write or full-access"),
            ("lifetime", "independent for human CLI spawns"),
        ],
        "verify-steering" => &[("model", MODEL), ("effort", EFFORT)],
        "setup" => &[
            ("codex_model", MODEL),
            ("claude_model", MODEL),
            ("codex_effort", EFFORT),
            ("claude_effort", EFFORT),
            ("provider", "codex or claude, separated by commas"),
            ("default_provider", "codex or claude"),
            ("login", "browser, device or skip"),
        ],
        "send" => &[
            ("mode", "next-turn, steer or redirect"),
            ("timeout", "an integer from 1 to 600 seconds"),
            ("delay", "a positive integer followed by s, m, h, d or w"),
        ],
        "wait" => &[
            ("until", "completed or turn-finished"),
            ("timeout", "an integer from 1 to 3600 seconds"),
            ("after", "an unsigned integer event cursor"),
        ],
        "approvals" => &[
            ("limit", "an integer from 1 to 200"),
            ("timeout", "an integer from 1 to 3600 seconds"),
        ],
        "batch" => &[(
            "deadline_seconds",
            "positive integer seconds whose sum with current Unix time fits 9223372036854775807",
        )],
        "gc" => &[(
            "older_than",
            "a positive integer followed by s, m, h, d or w",
        )],
        "agents" => &[
            ("after", "an unsigned integer agent cursor"),
            ("limit", "an integer from 1 to 200 rows"),
            ("stalled", "a positive integer followed by s, m, h, d or w"),
        ],
        "read" => &[
            ("after", "an unsigned integer cursor"),
            ("limit", "1..200 rows, or 4..65536 bytes for an artifact"),
        ],
        "messages" => &[
            ("before", "an unsigned integer message cursor"),
            ("limit", "an integer from 1 to 200 rows"),
        ],
        "output" => &[
            ("offset", "an unsigned integer byte offset"),
            ("limit", "an integer from 4 to 65536 bytes"),
        ],
        "compact" => &[
            ("before", "a Unix timestamp in 0..9223372036854775807"),
            ("keep_per_agent", "an event count in 1..9223372036854775807"),
        ],
        _ => &[],
    }
}

fn has(command: &clap::Command, id: &str) -> bool {
    command
        .get_arguments()
        .any(|arg| arg.get_id().as_str() == id)
}
