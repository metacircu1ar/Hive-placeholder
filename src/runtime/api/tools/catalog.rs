//! Trusted complete tool examples; no caller data enters diagnostic metadata.
use crate::error::Error;
use hive_input::Code;

pub(super) fn example(name: &str) -> &'static str {
    match name {
        "hive_spawn" => {
            "{\"name\":\"hive_spawn\",\"arguments\":{\"role\":\"reviewer\",\"body\":\"Review \
                the work.\",\"request_id\":\"spawn-1\"}}"
        }
        "hive_send" => {
            "{\"name\":\"hive_send\",\"arguments\":{\"target\":\"AGENT_ID\",\"body\":\"Review \
                the fix.\",\"request_id\":\"send-1\"}}"
        }
        "hive_batch" => {
            "{\"name\":\"hive_batch\",\"arguments\":{\"operations\":[{\"op\":\"send\",\
            \"target\":\"AGENT_ID\",\"body\":\"Review the work.\",\"request_id\":\"member-1\"}\
            ],\"request_id\":\"batch-1\"}}"
        }
        "hive_set_auth" => {
            "{\"name\":\"hive_set_auth\",\"arguments\":{\"target\":\"AGENT_ID\",\"auth\":\
            \"codex-main\",\"request_id\":\"auth-1\"}}"
        }
        "hive_set_name" => {
            "{\"name\":\"hive_set_name\",\"arguments\":{\"target\":\"AGENT_ID\",\"name\":\
            \"reviewer\",\"request_id\":\"name-1\"}}"
        }
        "hive_options_catalog" => {
            r#"{"name":"hive_options_catalog","arguments":{"provider":"codex"}}"#
        }
        "hive_set_options" => {
            "{\"name\":\"hive_set_options\",\"arguments\":{\"target\":\"AGENT_ID\",\"options\":\
            {\"model\":\"sonnet\",\"effort\":\"low\"},\"request_id\":\"options-1\"}}"
        }
        "hive_ask" => {
            "{\"name\":\"hive_ask\",\"arguments\":{\"message\":\"May I publish the \
                reviewed change?\",\"request_id\":\"ask-1\"}}"
        }
        "hive_complete" => {
            "{\"name\":\"hive_complete\",\"arguments\":{\"body\":\"Implemented and \
                validated.\",\"request_id\":\"done-1\"}}"
        }
        "hive_terminate" => {
            "{\"name\":\"hive_terminate\",\"arguments\":{\"target\":\"AGENT_ID\",\
            \"request_id\":\"terminate-1\"}}"
        }
        "hive_agents" => r#"{"name":"hive_agents","arguments":{"limit":100}}"#,
        "hive_read" => {
            r#"{"name":"hive_read","arguments":{"target":"AGENT_ID","after":0,"limit":100}}"#
        }
        "hive_messages" => {
            r#"{"name":"hive_messages","arguments":{"target":"AGENT_ID","limit":100}}"#
        }
        #[cfg(feature = "comb")]
        "hive_evidence" => {
            "{\"name\":\"hive_evidence\",\"arguments\":{\"kind\":\"review\",\"summary\":\
            \"Input validation is missing.\",\"verdict\":\"findings\",\"request_id\":\
            \"evidence-1\"}}"
        }
        // Unknown tools get a safe known example, never the supplied name.
        _ => r#"{"name":"hive_status","arguments":{"target":"AGENT_ID"}}"#,
    }
}

pub(crate) fn error(name: &str, code: Code, field: &str, expected: &str) -> Error {
    hive_input::InputError::new(
        code,
        "Invalid tool arguments.",
        field,
        expected,
        vec![example(name).into()],
    )
    .map_or_else(
        |cause| Error::internal_error("Invalid tool input metadata.").with_source(cause),
        Error::from,
    )
}

/// Keep the original cause while replacing operation examples with the tool call.
pub(super) fn contextualize(mut error: Error, name: &str) -> Error {
    if let Error::Input { input, .. } = &mut error {
        match hive_input::InputError::new(
            input.code(),
            input.message(),
            input.field(),
            input.expected(),
            vec![example(name).into()],
        ) {
            Ok(mapped) => **input = mapped,
            Err(cause) => {
                return Error::internal_error("Invalid tool input metadata.").with_source(cause);
            }
        }
    }
    error
}

#[cfg(feature = "comb")]
pub(super) const HIVE_EVIDENCE_DESCRIPTION: &str = "Submit explicitly public review evidence \
    to configured Comb collectors. Supply kind=review, verdict=findings|no_findings, summary, \
    relative files and optional commit. Corrections are human-only. No secrets or private \
    paths. Reuse request_id for retries; admission reports queued/full/disabled/not_selected \
    destinations.";

pub(super) const HIVE_COMPLETE_DESCRIPTION: &str = "Explicitly complete your task only after \
    required work and review are finished. Completion occurs when this turn ends \
    successfully. Give the result and validation evidence. Refused while a join you requested \
    is collecting before its deadline or its result is unread: end your turn instead; the \
    result arrives as new input.";

pub(super) const HIVE_SET_NAME_DESCRIPTION: &str = "Change an agent's display name at once, or \
    remove it with clear: true. Names label agents for people: they are not unique and never \
    select an agent, so keep using agent IDs as targets. Reuse request_id for retries.";

pub(super) const HIVE_SET_OPTIONS_DESCRIPTION: &str = "Persist a partial catalog options map \
    for future turns. Null resets to the frozen spawn value. Model and effort are ordinary \
    option keys. Applies only at a turn boundary, never mid-turn or through steer; it does \
    not send input or resume a paused agent. Use hive_options_catalog for supported keys. \
    Optional config is strict native text up to 64 KiB; model/effort and Hive-owned keys are \
    refused there. Native values stay private. Omitted config retains the overlay, supplied \
    config replaces it, and empty TOML or {} clears it. Reuse request_id with identical input \
    for retries.";

pub(super) const HIVE_ASK_DESCRIPTION: &str = "Ask the human an advisory question and receive \
    approval_id immediately. message is required; optional schema is a flat MCP elicitation \
    form object (at most 16 fields, 64 choices per field, 16 KiB schema). No schema means \
    plain accept/decline/cancel confirmation. message is limited to 16 KiB; title to 256 \
    bytes. destructive is a presentation hint, not enforcement. Never request passwords, \
    tokens, API keys or other secrets. A new ask supersedes your pending question. Use a new \
    request_id for every new question; reuse it only to retry the same call, never for a \
    later question. Replays return the original result even after closure. After asking, \
    finish your current provider turn; do not call hive_complete, which ends your task and \
    withdraws the question. The human's answer arrives as a new input.";

pub(super) const HIVE_BATCH_DESCRIPTION: &str = "Atomically accept up to 64 \
    spawn/send/terminate operations. Each operation needs its own stable request_id. Commit \
    before acknowledgement; reuse identical IDs when retrying. Optional join:{} collects each \
    message turn's final answer into one new input; deadline_seconds emits a partial result \
    then continues collecting. Join sends must be next-turn to other agents, with fresh \
    operation IDs. Ask recipients to answer in their final output, not send a separate reply. \
    End your turn after acceptance; do not complete your task or wait in a tool. Read \
    retained results with hive_read target=join:ID.";

pub(super) const HIVE_SPAWN_DESCRIPTION: &str = "Launch a new Hive agent with a separate \
    conversation. Optional from_agent inherits that agent's native history through its \
    current turn, or latest finished turn when idle. It returns immediately; finish your turn \
    when branching yourself. The source continues independently and the workspace is shared \
    unless cwd changes. The new agent uses the same provider and login. It becomes your \
    child. Prefer a configured role when it fits; otherwise provider defaults and explicit \
    overrides choose the settings. Empty instructions clears role instructions. lifetime \
    defaults to the role: independent survives your end; with_parent expires when you \
    complete, terminate or expire. sandbox accepts read-only, workspace-write or full-access; \
    wider children require runtime.allow_spawn_widening=true in the daemon configuration. \
    Branches retain their source sandbox. Submit this turn's batch of independent work, then \
    end your turn; replies activate your next turn. A joined startup captures the first \
    turn's final answer as its reply. Never use built-in provider delegation tools.";

pub(super) const HIVE_SEND_DESCRIPTION: &str = "Send durable input to an existing agent. \
    target accepts an agent ID, @parent, or @child when you have exactly one retained direct \
    child (including ended children). mode defaults to next-turn; steer guides its active \
    turn without starting a new turn; redirect interrupts the current turn and puts guidance \
    before ordinary queued input on the same conversation. delay_seconds schedules next-turn \
    input in 1 to 604800 seconds (7 days); end your turn afterwards. Startup stays first. \
    Acceptance is not provider delivery. Reuse the same reviewer ID for later rounds. After \
    handing work off, end your turn; replies automatically start your next turn. Joined \
    requests collect the consuming turn's final answer; an explicit send back is separate.";

pub(super) const HIVE_SET_AUTH_DESCRIPTION: &str = concat!(
    "Persist a subscription/API-key profile or fallback group for the next turn boundary. ",
    "Keeps the agent identity and mailbox; a different login uses a fresh ",
    "conversation with a bounded handoff. Does not interrupt the current turn. ",
    "Reuse request_id for retries.",
);

pub(super) const HIVE_TERMINATE_DESCRIPTION: &str = "Permanently cancel one agent; target \
    accepts an agent ID, @parent, or @child when you have exactly one retained direct child \
    (including ended children). Discard queued input and request asynchronous session \
    cleanup. It cannot resume. Recursively expire with_parent descendants; independent \
    children and their subtrees survive. Shared Codex processes may remain for other \
    conversations or their linger period; Claude uses one process per conversation. \
    Acceptance does not confirm process exit.";

pub(super) const HIVE_OPTIONS_CATALOG_DESCRIPTION: &str = "List per-agent option keys, types, \
    scopes, defaults, model constraints and native mappings. Use this before selecting \
    options or supplying a native overlay.";

pub(super) const HIVE_STATUS_DESCRIPTION: &str = "Inspect any Hive agent's process and task \
    state.";

pub(super) const HIVE_AGENTS_DESCRIPTION: &str = "List compact agent summaries. Page with \
    next_after as after until the page is empty. Flat visibility; optional parent/auth \
    filters. stalled_seconds instead lists whole quiet live groups with empty mailboxes and \
    no pending approvals or auth waits. Default 100 groups, max 200. Page with next_cursor as \
    stalled_after until null; cannot combine stalled mode with after/parent/auth.";

pub(super) const HIVE_READ_DESCRIPTION: &str = "Read events by cursor (limit 1..200 rows, \
    default 100), or a retained join snapshot with target=join:ID and no after/limit. For a \
    large event artifact, use its read_target with after=byte offset and limit 4..65536 bytes \
    (default 65536); follow next_offset until eof.";

pub(super) const HIVE_MESSAGES_DESCRIPTION: &str = "Read messages to/from an agent, newest \
    first. To read older pages, pass next_before as before until messages is empty. Default \
    limit 100; maximum 200.";
