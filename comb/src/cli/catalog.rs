//! Command examples shared by help, parser errors and the product guide.
mod help;
use hive_input::Examples;

pub(super) const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "upgrade",
        "Inspect and upgrade a stopped knowledge root",
        "comb upgrade --dry-run",
    ),
    (
        "",
        "Shared local knowledge for people and agents",
        "comb --help",
    ),
    (
        "service",
        "Manage this root's user login service",
        "comb service status",
    ),
    (
        "service install",
        "Install this root's user login service",
        "comb service install",
    ),
    (
        "service start",
        "Start the installed service and wait for readiness",
        "comb service start",
    ),
    (
        "service stop",
        "Stop the owned user service",
        "comb service stop",
    ),
    (
        "service status",
        "Inspect the installed service and its readiness",
        "comb service status",
    ),
    (
        "service uninstall",
        "Remove the user service while retaining knowledge",
        "comb service uninstall",
    ),
    (
        "setup",
        "Prepare knowledge, embeddings and optional Hive integration",
        comb::setup::input::EXAMPLE,
    ),
    (
        "version",
        "Describe the installed binary and embedding backends",
        "comb version --json",
    ),
    (
        "model",
        "Select text search only or an HTTP embedding server",
        "comb model use none",
    ),
    ("model use", "Serve text search only", "comb model use none"),
    (
        "model use-http",
        "Validate and select an external embedding server",
        "comb model use-http --url http://127.0.0.1:8081/v1/embeddings --model \
            embedding-model --dimensions 1024",
    ),
    (
        "serve",
        "Serve shared knowledge and index embeddings in the background",
        "comb --root /private/knowledge serve",
    ),
    (
        "mcp",
        "Expose agent tools over the MCP stdio protocol",
        "comb mcp --credential /private/client.json",
    ),
    (
        "logs",
        "Tail and filter structured diagnostics without opening knowledge state",
        "comb --root /private/knowledge logs --tail 100",
    ),
    (
        "credential",
        "Manage credentials for knowledge clients",
        "comb credential list",
    ),
    (
        "credential create",
        "Issue a client credential and write its private connection file",
        "comb credential create reader --out /private/client.json",
    ),
    (
        "credential list",
        "List credential identities and permissions without secrets",
        "comb credential list",
    ),
    (
        "credential revoke",
        "Revoke one credential identity",
        "comb credential revoke 00000000-0000-4000-8000-000000000001",
    ),
    (
        "init",
        "Initialize a knowledge root and its text index",
        "comb init /private/knowledge",
    ),
    (
        "get",
        "Read a page, its evidence and current hash",
        "comb get components/writer",
    ),
    (
        "put",
        "Create a page or replace it using its current hash",
        "comb put components/writer --file writer.md --request-id create-writer",
    ),
    (
        "append",
        "Append evidence, optionally replacing the summary atomically",
        "comb append components/writer Verified --source commit:abc123 --request-id evidence-1",
    ),
    (
        "capture",
        "Write a new inbox note",
        "comb capture Investigate --request-id note-1",
    ),
    (
        "promote",
        "Mark a page's current summary and evidence reviewed",
        "comb promote components/writer --expected-hash \
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
    (
        "consolidate",
        "Fold inbox evidence into a target page and record a receipt",
        "comb --run curate-1 consolidate inbox/note components/writer --note-hash \
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
            --target-hash \
            bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
            --summary-file summary.md",
    ),
    (
        "decide",
        "Record an explicit decision on a review proposal",
        "comb decide review/proposal accept --reason Verified --expected-hash \
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
    (
        "rm",
        "Remove a page while preserving deletion retry receipts",
        "comb rm components/obsolete --expected-hash \
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
            --request-id remove-1",
    ),
    (
        "list",
        "Filter and paginate knowledge pages",
        "comb list --prefix components/ --limit 20",
    ),
    (
        "search",
        "Retrieve pages with literal or semantic search",
        "comb search WriterLoop --mode text --limit 10",
    ),
    (
        "embed",
        "Activate embeddings and finish an offline backfill",
        "comb embed --config embeddings.yaml",
    ),
    (
        "links",
        "List outgoing page links",
        "comb links components/writer --limit 20",
    ),
    (
        "backlinks",
        "List pages linking to a page",
        "comb backlinks components/writer --limit 20",
    ),
    (
        "graph",
        "Read a bounded undirected page neighborhood",
        "comb graph components/writer --depth 2",
    ),
    (
        "import",
        "Import Markdown as human-reviewed knowledge",
        "comb import ./notes --prefix operations",
    ),
    (
        "sync",
        "Reconcile external edits or rebuild the derived index",
        "comb sync --rebuild",
    ),
    (
        "doctor",
        "Report knowledge, indexing and citation problems",
        "comb doctor",
    ),
    (
        "stats",
        "Show page, chunk, link and backlog counts",
        "comb stats",
    ),
];

pub(super) fn examples(path: &str) -> Option<Examples> {
    // Installer maintenance is hidden, but malformed invocations still have a
    // safe context. Public help points users back to supported setup commands.
    let path = if path == "setup-maintenance" {
        "setup"
    } else {
        path
    };
    COMMANDS
        .iter()
        .find(|(key, _, _)| *key == path)
        .and_then(|(_, _, example)| Examples::new(vec![(*example).to_owned()]).ok())
}

pub(super) fn decorate(mut command: clap::Command, path: &str) -> clap::Command {
    if let Some(examples) = examples(path) {
        command = command.after_help(examples.help());
    }
    if command.get_about().is_none()
        && let Some((_, about, _)) = COMMANDS.iter().find(|(key, _, _)| *key == path)
    {
        command = command.about(*about);
    }
    command = help::arguments(command, path);
    command.mut_subcommands(|child| {
        let path = if path.is_empty() {
            child.get_name().to_owned()
        } else {
            format!("{path} {}", child.get_name())
        };
        decorate(child, &path)
    })
}
