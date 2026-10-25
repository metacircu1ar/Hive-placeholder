//! Lifecycle regressions.
use super::*;

#[test]
fn complete_ignores_agent_target_and_checks_lifecycle_before_body() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    let accepted = call(
        &mut core,
        "caller",
        "complete",
        json!({"op":"complete","target":{"invalid":true},"body":"done"}),
    )
    .unwrap();
    assert_eq!(
        accepted,
        json!({"agent_id":"caller","completion":"pending_turn_boundary"})
    );
    assert_eq!(
        core.store()
            .agent("caller")
            .unwrap()
            .completion_pending
            .as_deref(),
        Some("done")
    );
    assert_error(
        call(
            &mut core,
            "caller",
            "pending",
            json!({"op":"complete","body":false}),
        ),
        Error::completion_pending("Completion is already pending the turn boundary."),
    );
    assert_error(
        call(
            &mut core,
            "human",
            "human-target",
            json!({"op":"complete","target":false,"body":"done"}),
        ),
        InputContext::Complete.error(
            hive_input::Code::InvalidValue,
            "/target",
            "a string within 1..65536 UTF-8 bytes",
        ),
    );
    assert_error(
        call(
            &mut core,
            "human",
            "unknown",
            json!({"op":"complete","target":"absent"}),
        ),
        Error::unknown_agent("No agent has this ID."),
    );
    let mut caller = core.store().agent("caller").unwrap();
    caller.task_outcome = TaskOutcome::Completed;
    core.store().save(&caller).unwrap();
    assert_error(
        call(
            &mut core,
            "human",
            "terminal",
            json!({"op":"complete","target":"caller"}),
        ),
        completed(),
    );
}

#[test]
fn roles_list_spawn_choices_with_providers_for_humans_only() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut c = core(tmp.path());
    let parsed = crate::config::Config::parse(concat!(
        "auth_profiles:\n",
        "  main: {provider: codex, method: subscription}\n",
        "  claude-main: {provider: claude, method: subscription}\n",
        "auth_groups: {plans: {mode: ordered, members: [main]}}\n",
        "providers: {codex: {auth: main, model: terra}, claude: {auth: claude-main, model: sonnet}}\n",
        "roles: {reviewer: {auth: claude-main}, worker: {provider: codex}}\n",
    ))
    .unwrap();
    let config = c.config_mut();
    config.auth_profiles = parsed.auth_profiles;
    config.auth_groups = parsed.auth_groups;
    config.roles = parsed.roles;
    config.default_provider = None;
    let roles = pagination::observe(&c, json!({"op":"roles"})).unwrap();
    assert_eq!(
        roles,
        json!({
            "default_provider": null,
            "roles": [
                {"name": "reviewer", "provider": "claude"},
                {"name": "worker", "provider": "codex"},
            ],
            "profiles": [
                {"name": "claude-main", "provider": "claude"},
                {"name": "main", "provider": "codex"},
            ],
            "groups": [{"name": "plans", "provider": "codex"}],
        })
    );
    c.config_mut().default_provider = Some(crate::config::Provider::Codex);
    let roles = pagination::observe(&c, json!({"op":"roles"})).unwrap();
    assert_eq!(roles["default_provider"], "codex");
    let request = RequestEnvelope::new(json!({"op":"roles"}));
    assert!(request.request.is_read_only());
    assert!(request.request.caller_policy(&Caller::Human).is_ok());
    for caller in [Caller::test_agent("agent"), Caller::System] {
        assert!(matches!(
            c.dispatch(&caller, "roles", &request, &Facts::default()),
            Err(Error::UnsupportedOperation { .. })
        ));
    }
}
