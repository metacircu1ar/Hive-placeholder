use super::*;
use crate::runtime::test_support::{RuntimeFixture, WriterGate};
use serde_json::json;
use std::time::Duration;

pub(super) async fn configured() -> RuntimeFixture {
    let runtime = RuntimeFixture::new();
    std::fs::write(runtime.temp.path().join("role.md"), "test role").unwrap();
    let catalogs = runtime.hub.catalogs.clone();
    let binary = runtime.temp.path().join("codex");
    crate::runtime::test_support::codex_binary(&binary, "");
    runtime
        .hub
        .with(move |core| {
            let agent = core.store().agent("agent")?;
            *core.binaries_mut() = crate::runtime::Binaries {
                codex: binary.into(),
                claude: "/bin/echo".into(),
            }
            .resolved();
            let binary = core.binaries().resolve(agent.profile.provider)?;
            let identity = crate::runtime::provider::catalogs::BinaryIdentity::read(&binary)?;
            let model = crate::runtime::provider::protocol::Model::parse(
                &json!({"model":"mock","supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
            );
            catalogs.observe(agent.auth_generation, None, identity, vec![model]);
            core.config_mut()
                .auth_profiles
                .insert("main".into(), agent.profile);
            core.config_mut().roles.insert(
                "test".into(),
                serde_json::from_value(json!({
                    "auth":"main", "model":"mock", "instructions":"role.md"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .await
        .unwrap();
    runtime
}

#[tokio::test]
async fn replayed_inner_spawn_bypasses_a_blocked_filesystem_service() {
    let runtime = configured().await;
    let request = json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":"task"});
    let accepted = runtime
        .hub
        .dispatch(
            Caller::Human,
            "inner".into(),
            RequestEnvelope::new(request.clone()),
        )
        .await
        .unwrap();
    std::fs::remove_file(runtime.temp.path().join("role.md")).unwrap();
    let (entered, running) = tokio::sync::oneshot::channel();
    let (release, blocked) = WriterGate::new();
    let service = runtime.hub.preflight.clone();
    let held = tokio::spawn(async move { service.hold(entered, blocked).await });
    running.await.unwrap();
    let mut item = request;
    item["request_id"] = json!("inner");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        runtime.hub.dispatch(
            Caller::Human,
            "outer".into(),
            RequestEnvelope::new(json!({"op":"batch","operations":[item]})),
        ),
    )
    .await;
    release.release();
    held.await.unwrap().unwrap();
    assert_eq!(result.unwrap().unwrap()["results"][0], accepted);
    runtime.finish();
}

#[tokio::test]
async fn selected_generation_remains_pinned_across_repair_and_prune_is_excluded() {
    let runtime = configured().await;
    let request = RequestEnvelope::new(
        json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":"task"}),
    );
    let copy = request.clone();
    let input = runtime
        .hub
        .with(move |core| Ok(RequestInput::capture(core, &Caller::Human, &copy)))
        .await
        .unwrap();
    let mut facts = runtime
        .hub
        .preflight
        .request(Box::new(input))
        .await
        .unwrap();
    facts.discover(&runtime.hub.catalogs).await;
    let (root, old, profile) = runtime
        .hub
        .with(|core| {
            let agent = core.store().agent("agent")?;
            Ok((
                core.state().root.clone(),
                agent.auth_generation,
                agent.profile,
            ))
        })
        .await
        .unwrap();
    let state = crate::state::State::new(root).unwrap();
    let repair = state.begin_repair("main", &profile).unwrap();
    state.finish_repair(&repair, true, None).unwrap();
    assert_eq!(state.prune("main", old).unwrap_err().code(), "daemon_busy");
    let result = runtime
        .hub
        .with(move |core| core.dispatch(&Caller::Human, "pin", &request, &facts))
        .await
        .unwrap();
    let id = result["agent_id"].as_str().unwrap().to_owned();
    let generation = runtime
        .hub
        .with(move |core| Ok(core.store().agent(&id)?.auth_generation))
        .await
        .unwrap();
    assert_eq!(generation, old);
    assert!(state.generation("main", &profile, old).is_ok());
    runtime.finish();
}

#[tokio::test]
async fn resume_proof_must_match_current_conversation_identity() {
    let runtime = configured().await;
    let request = RequestEnvelope::new(json!({"op":"resume","target":"agent"}));
    let input = runtime
        .hub
        .with(move |core| Ok(RequestInput::capture(core, &Caller::Human, &request)))
        .await
        .unwrap();
    let facts = runtime
        .hub
        .preflight
        .request(Box::new(input))
        .await
        .unwrap();
    assert!(
        runtime
            .hub
            .with(move |core| {
                assert!(facts.fresh(core)?);
                let mut agent = core.store().agent("agent")?;
                agent.provider_process = Some(crate::runtime::domain::ProviderProcess::Reserved {
                    id: "different-provider".into(),
                });
                core.store().save(&agent)?;
                Ok(!facts.fresh(core)?)
            })
            .await
            .unwrap()
    );
    runtime.finish();
}

#[tokio::test]
async fn missing_preflight_facts_fail_the_request_without_panicking_the_writer() {
    let runtime = configured().await;
    let cwd = runtime.temp.path().to_owned();
    let errors = runtime
        .hub
        .with(move |core| {
            let spawn =
                RequestEnvelope::new(json!({"op":"spawn","role":"test","cwd":cwd,"body":"task"}));
            let resume = RequestEnvelope::new(json!({"op":"resume","target":"agent"}));
            Ok([
                core.dispatch(&Caller::Human, "missing-spawn", &spawn, &Facts::default())
                    .unwrap_err()
                    .code(),
                core.dispatch(&Caller::Human, "missing-resume", &resume, &Facts::default())
                    .unwrap_err()
                    .code(),
            ])
        })
        .await
        .unwrap();
    assert_eq!(errors, ["internal_error", "internal_error"]);
    assert_eq!(
        runtime
            .hub
            .with(|core| Ok(core.store().agent("agent")?.id))
            .await
            .unwrap(),
        "agent"
    );
    runtime.finish();
}

#[tokio::test]
async fn approvals_ask_is_fenced_by_execution_and_turn_even_on_replay() {
    let runtime = RuntimeFixture::new();
    let request = RequestEnvelope::new(json!({"op":"ask","message":"Proceed?"}));
    for (execution, turn) in [("stale", "turn"), ("execution", "stale")] {
        let caller = Caller::Agent {
            id: "agent".into(),
            execution: execution.into(),
            turn: Some(turn.into()),
        };
        assert_eq!(
            runtime
                .hub
                .dispatch(caller, "ask".into(), request.clone())
                .await
                .unwrap_err()
                .code(),
            "stale_execution"
        );
    }
    let caller = Caller::Agent {
        id: "agent".into(),
        execution: "execution".into(),
        turn: Some("turn".into()),
    };
    let asked = runtime
        .hub
        .dispatch(caller.clone(), "ask".into(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        runtime
            .hub
            .dispatch(caller.clone(), "ask".into(), request.clone())
            .await
            .unwrap(),
        asked
    );
    runtime
        .hub
        .with(|core| {
            let mut a = core.store().agent("agent")?;
            a.turn_id = Some("new-turn".into());
            core.store().save(&a)
        })
        .await
        .unwrap();
    assert_eq!(
        runtime
            .hub
            .dispatch(caller, "ask".into(), request)
            .await
            .unwrap_err()
            .code(),
        "stale_execution"
    );
    runtime.finish();
}

#[tokio::test]
async fn approvals_ask_without_an_active_turn_reports_lifecycle_not_schema_error() {
    let runtime = RuntimeFixture::new();
    for (execution, turn, running) in [
        (Some("execution"), None, true),
        (None, Some("turn"), true),
        (Some("execution"), Some("turn"), false),
    ] {
        runtime
            .hub
            .with(move |core| {
                let mut a = core.store().agent("agent")?;
                a.execution_id = execution.map(str::to_owned);
                a.turn_id = turn.map(str::to_owned);
                a.state = if running {
                    crate::runtime::domain::AgentState::Running
                } else {
                    crate::runtime::domain::AgentState::Idle
                };
                core.store().save(&a)
            })
            .await
            .unwrap();
        let request = RequestEnvelope::new(json!({"op":"ask","message":"Proceed?"}));
        let error = if execution.is_none() {
            // Hub correctly rejects a missing execution as stale; directly cover the
            // core guard as well without fabricating caller ownership.
            runtime
                .hub
                .with(move |core| {
                    core.dispatch(
                        &Caller::test_agent("agent"),
                        "ask",
                        &request,
                        &Facts::default(),
                    )
                })
                .await
                .unwrap_err()
        } else {
            runtime
                .hub
                .dispatch(
                    Caller::Agent {
                        id: "agent".into(),
                        execution: "execution".into(),
                        turn: turn.map(str::to_owned),
                    },
                    "ask".into(),
                    request,
                )
                .await
                .unwrap_err()
        };
        assert_eq!(error.code(), "no_active_turn");
    }
    runtime.finish();
}

#[tokio::test]
async fn cancellation_retains_evidence_until_outer_commit() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Evidence {
        path: std::path::PathBuf,
        committed: Arc<AtomicBool>,
    }
    impl Drop for Evidence {
        fn drop(&mut self) {
            let connection = rusqlite::Connection::open(&self.path).unwrap();
            let committed: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM agents WHERE id='held')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            self.committed.store(committed, Ordering::SeqCst);
        }
    }
    let runtime = RuntimeFixture::new();
    let committed = Arc::new(AtomicBool::new(false));
    let evidence = Evidence {
        path: runtime.temp.path().join("runtime.sqlite"),
        committed: committed.clone(),
    };
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = WriterGate::new();
    let hub = runtime.hub.clone();
    let caller = tokio::spawn(async move {
        hub.with_retained(evidence, move |core| {
            let mut agent = core.store().agent("agent")?;
            agent.id = "held".into();
            core.store().save(&agent)?;
            entered.send(()).unwrap();
            blocked.recv().unwrap();
            Ok(())
        })
        .await
    });
    started.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(!committed.load(Ordering::SeqCst));
    release.release();
    runtime.hub.with(|_| Ok(())).await.unwrap();
    assert!(committed.load(Ordering::SeqCst));
    runtime.finish();
}

#[tokio::test]
async fn invalid_local_spawn_is_rejected_before_unavailable_catalog_capacity() {
    let runtime = configured().await;
    runtime.hub.catalogs.invalidate(runtime.lease.generation());
    for (request, expected, field) in [
        (
            json!({"op":"spawn","role":"test","cwd":"relative","body":"task"}),
            "invalid_path",
            Some("/cwd"),
        ),
        (
            json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":"task","lifetime":"with_parent"}),
            "parent_required",
            None,
        ),
        (
            json!({"op":"spawn","role":"test","cwd":runtime.temp.path()}),
            "missing_argument",
            Some("/body"),
        ),
        (
            json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":""}),
            "out_of_range",
            Some("/body"),
        ),
        (
            json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":5}),
            "invalid_value",
            Some("/body"),
        ),
        (
            json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":"x".repeat(65537)}),
            "out_of_range",
            Some("/body"),
        ),
        (
            json!({"op":"spawn","role":"test","cwd":runtime.temp.path(),"body":"task","instructions":"x".repeat(65537)}),
            "out_of_range",
            Some("/instructions"),
        ),
    ] {
        let result = runtime
            .hub
            .dispatch(
                Caller::Human,
                uuid::Uuid::new_v4().to_string(),
                RequestEnvelope::new(request),
            )
            .await;
        let error = result.unwrap_err();
        assert_eq!(error.code(), expected);
        if let Some(field) = field {
            assert!(matches!(error, Error::Input { .. }));
            assert_eq!(serde_json::to_value(error).unwrap()["field"], field);
        }
    }
    runtime.finish();
}

mod attachments;
