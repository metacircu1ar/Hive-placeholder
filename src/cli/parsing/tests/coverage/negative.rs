use super::inventory::{
    Row,
    Rule::{Deferred, Reject},
};

const PATH: &str = "Filesystem paths are accepted lexically; ownership, existence and content are checked before use.";
const TEXT: &str = "Literal payload text is checked for size and content during request preparation, before daemon dispatch.";
const SETUP: &str = "Setup values also come from answers files; the resolved plan validates them before applying changes.";

pub(super) const ROWS: &[Row] = &[
    (
        "@global",
        "config state_dir codex_bin claude_bin",
        Deferred(PATH),
    ),
    ("@global", "probe_timeout login_timeout", Reject("bad")),
    ("agents", "after limit stalled", Reject("bad")),
    ("agents", "parent auth", Reject("")),
    ("agents", "stalled_after", Reject("invalid-json")),
    ("approvals", "limit timeout", Reject("bad")),
    ("approvals", "agent", Reject("")),
    (
        "approvals",
        "after",
        Deferred(
            "Approval cursor JSON is decoded and validated during request preparation before daemon dispatch.",
        ),
    ),
    ("approvals show|respond", "approval_id", Reject("bad-uuid")),
    (
        "auth generations|auth login|auth prune|auth recover|auth repair|auth status|auth usage",
        "name",
        Reject(""),
    ),
    ("auth history-export", "output", Deferred(PATH)),
    ("auth prune", "generation", Reject("bad-uuid")),
    ("auth repair", "new_state_dir", Deferred(PATH)),
    ("batch", "operations_file", Deferred(PATH)),
    ("batch", "deadline_seconds", Reject("bad")),
    (
        "batch|complete|interrupt|purge|respond|resume|send|set-auth|set-name|set-options|spawn|stop|terminate",
        "request_id",
        Reject(""),
    ),
    ("compact", "before keep_per_agent", Reject("bad")),
    ("complete", "body", Deferred(TEXT)),
    ("complete", "result_file", Deferred(PATH)),
    ("gc", "older_than", Reject("bad")),
    ("logs", "tail level", Reject("bad")),
    ("logs", "agent event", Reject("")),
    (
        "attach|complete|interrupt|live|messages|purge|read|resume|send|set-auth|set-name|set-options|status|stop|terminate|wait",
        "target",
        Reject(""),
    ),
    ("messages", "before limit", Reject("bad")),
    ("models|verify-steering", "auth", Reject("")),
    ("output", "id", Reject("bad-uuid")),
    ("output", "offset limit", Reject("bad")),
    ("read", "after limit", Reject("bad")),
    ("live", "wait_ms", Reject("bad")),
    ("panes", "root", Reject("")),
    ("panes", "limit linger backend", Reject("bad")),
    ("attach", "linger", Reject("bad")),
    ("tui", "root", Reject("")),
    ("tui", "limit", Reject("bad")),
    ("respond", "data_file", Deferred(PATH)),
    ("respond", "data reason", Deferred(TEXT)),
    ("send", "attachments body_file", Deferred(PATH)),
    ("send", "body", Deferred(TEXT)),
    ("send", "delay timeout mode", Reject("bad")),
    (
        "service install",
        "environment_file program",
        Deferred(PATH),
    ),
    (
        "service install",
        "service_path",
        Deferred(
            "Service PATH entries are validated against the selected platform before writing the service definition.",
        ),
    ),
    ("set-auth", "auth", Reject("")),
    ("set-name|spawn", "name", Reject("")),
    ("verify-steering", "model effort", Reject("")),
    ("options catalog", "provider", Reject("bad")),
    ("options catalog", "model", Reject("")),
    ("spawn|set-options", "native_config", Deferred(PATH)),
    (
        "spawn|set-options",
        "option",
        Deferred("KEY=VALUE assignments are parsed and catalog-validated before daemon dispatch."),
    ),
    (
        "setup",
        "answers bridge_file embedding_config comb_bin comb_root prefix purge_comb_root service_environment skills_dir",
        Deferred(PATH),
    ),
    (
        "setup",
        "provider default_provider login install_provider user_skills",
        Reject("bad"),
    ),
    (
        "setup",
        "codex_model claude_model codex_effort claude_effort",
        Reject(""),
    ),
    (
        "setup",
        "collectors dry_run no_comb no_comb_roles no_services purge restart_daemon services uninstall yes",
        Reject("bad"),
    ),
    (
        "setup",
        "codex_profile claude_profile service_path",
        Deferred(SETUP),
    ),
    (
        "spawn",
        "attachments body_file cwd instructions_file",
        Deferred(PATH),
    ),
    ("spawn", "body", Deferred(TEXT)),
    ("spawn", "role preset from auth model effort", Reject("")),
    ("spawn", "provider lifetime sandbox", Reject("bad")),
    ("wait", "after timeout until", Reject("bad")),
    ("wait", "message join", Reject("")),
    #[cfg(feature = "comb")]
    ("collectors", "request_id", Reject("")),
    #[cfg(feature = "comb")]
    (
        "collectors discard|collectors rebind|collectors release-uncertain",
        "destination",
        Reject(""),
    ),
    #[cfg(feature = "comb")]
    ("collectors rebind", "comb_id", Reject("bad-uuid")),
    #[cfg(feature = "comb")]
    ("collectors release-uncertain", "item", Reject("")),
    #[cfg(feature = "comb")]
    ("evidence", "request_id", Reject("")),
    #[cfg(feature = "comb")]
    ("evidence", "kind verdict", Reject("bad")),
    #[cfg(feature = "comb")]
    ("evidence", "summary", Deferred(TEXT)),
    #[cfg(feature = "comb")]
    ("evidence", "summary_file", Deferred(PATH)),
    #[cfg(feature = "comb")]
    (
        "evidence",
        "commit files",
        Deferred(
            "Evidence commit and file references are validated together with the evidence kind before body-file reads.",
        ),
    ),
];
