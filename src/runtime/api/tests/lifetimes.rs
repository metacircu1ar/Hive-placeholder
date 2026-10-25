use super::*;
use crate::{
    config::Lifetime,
    runtime::domain::{Control, MessageState},
};

fn child(core: &Core, id: &str, parent: &str, lifetime: Lifetime) {
    let mut a = agent(id);
    a.parent_id = Some(parent.into());
    a.lifetime = lifetime;
    a.completion_pending = Some("unfinished".into());
    core.store().save(&a).unwrap();
    core.store()
        .enqueue(&format!("input-{id}"), "human", id, "queued")
        .unwrap();
}

fn expirations(core: &Core, id: &str) -> Vec<Value> {
    core.store().events(id, 0, 200).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "lifetime_expired")
        .cloned()
        .collect()
}

fn assert_expired(core: &Core, id: &str, parent: &str, reason: &str) {
    let a = core.store().agent(id).unwrap();
    assert!(a.expired);
    assert!(a.completion_pending.is_none());
    let events = expirations(core, id);
    assert_eq!(events.len(), 1, "{id}: {events:?}");
    assert_eq!(
        events[0]["data"],
        json!({"parent_id":parent,"reason":reason})
    );
    assert_eq!(
        core.store().messages(id, None, 200).unwrap()["messages"][0]["state"],
        "undeliverable"
    );
}

#[test]
fn lifetimes_cascade_three_levels_for_completion_and_termination_preserving_independent_subtrees() {
    for complete in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut core = core(temp.path());
        core.store().save(&agent("root")).unwrap();
        child(&core, "first", "root", Lifetime::WithParent);
        child(&core, "second", "first", Lifetime::WithParent);
        child(&core, "third", "second", Lifetime::WithParent);
        child(&core, "free", "first", Lifetime::Independent);
        child(&core, "free-bound", "free", Lifetime::WithParent);
        let mut running = core.store().agent("first").unwrap();
        running.state = AgentState::Running;
        core.started(running.id.clone());
        core.store().save(&running).unwrap();
        if complete {
            call(
                &mut core,
                "human",
                "complete",
                json!({"op":"complete","target":"root","body":"done"}),
            )
            .unwrap();
            assert!(
                !core.store().agent("first").unwrap().expired,
                "completion is deferred"
            );
            let mut parent = core.store().agent("root").unwrap();
            core.atomic(|core| core.finish_task(&mut parent)).unwrap();
        } else {
            call(
                &mut core,
                "human",
                "terminate",
                json!({"op":"terminate","target":"root"}),
            )
            .unwrap();
        }
        assert_expired(
            &core,
            "first",
            "root",
            if complete {
                "parent_completed"
            } else {
                "parent_terminated"
            },
        );
        assert_expired(&core, "second", "first", "parent_expired");
        assert_expired(&core, "third", "second", "parent_expired");
        let first = core.store().agent("first").unwrap();
        assert_eq!(first.state, AgentState::Running);
        assert_eq!(first.control, Some(Control::Stop));
        assert!(core.running().contains("first"));
        assert_eq!(
            core.store().agent("second").unwrap().state,
            AgentState::Stopped
        );
        for id in ["free", "free-bound"] {
            assert!(!core.store().agent(id).unwrap().expired);
            assert!(core.store().next_message(id).unwrap().is_some());
            assert!(expirations(&core, id).is_empty());
        }
        call(
            &mut core,
            "human",
            "end-free",
            json!({"op":"terminate","target":"free"}),
        )
        .unwrap();
        assert_expired(&core, "free-bound", "free", "parent_terminated");
    }
}

#[test]
fn lifetimes_page_mixed_children_and_preserve_previous_outcomes_and_replay() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().begin().unwrap();
    core.store().save(&agent("root")).unwrap();
    for i in 0..260 {
        let id = format!("child-{i}");
        child(&core, &id, "root", Lifetime::WithParent);
        let mut a = core.store().agent(&id).unwrap();
        a.task_outcome = match i % 3 {
            0 => TaskOutcome::Open,
            1 => TaskOutcome::Completed,
            _ => TaskOutcome::Cancelled,
        };
        a.result = Some("recorded".into());
        core.store().save(&a).unwrap();
    }
    child(&core, "grandchild", "child-259", Lifetime::WithParent);
    core.store().commit().unwrap();
    let args = json!({"op":"terminate","target":"root"});
    let accepted = call(&mut core, "human", "end", args.clone()).unwrap();
    assert_eq!(
        call(&mut core, "human", "end", args.clone()).unwrap(),
        accepted
    );
    assert_error(call(&mut core, "human", "end-again", args), completed());
    for i in 0..260 {
        let id = format!("child-{i}");
        assert_expired(&core, &id, "root", "parent_terminated");
        let a = core.store().agent(&id).unwrap();
        assert_eq!(
            a.task_outcome,
            match i % 3 {
                0 => TaskOutcome::Open,
                1 => TaskOutcome::Completed,
                _ => TaskOutcome::Cancelled,
            }
        );
        assert_eq!(a.result.as_deref(), Some("recorded"));
    }
    assert_expired(&core, "grandchild", "child-259", "parent_expired");
    assert_error(
        call(
            &mut core,
            "human",
            "expired-end",
            json!({"op":"terminate","target":"grandchild"}),
        ),
        completed(),
    );
    let db = rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap();
    let mut plan = db.prepare("EXPLAIN QUERY PLAN SELECT id FROM agents WHERE parent_id='root' AND lifetime='with_parent' AND expired=0 ORDER BY listing_seq LIMIT 128").unwrap();
    let rows = plan
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        rows.iter().any(|row| row.contains("agents_dependents")),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("TEMP B-TREE")),
        "{rows:?}"
    );
}

#[test]
fn lifetimes_deep_chains_use_an_iterative_worklist() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().begin().unwrap();
    core.store().save(&agent("0")).unwrap();
    for i in 1..=1024 {
        child(
            &core,
            &i.to_string(),
            &(i - 1).to_string(),
            Lifetime::WithParent,
        );
    }
    core.store().commit().unwrap();
    call(
        &mut core,
        "human",
        "end",
        json!({"op":"terminate","target":"0"}),
    )
    .unwrap();
    assert_expired(&core, "1024", "1023", "parent_expired");
    assert_eq!(
        core.store().active_count().unwrap(),
        1,
        "root shutdown is still pending"
    );
}

#[test]
fn lifetimes_stop_pause_failure_and_submitted_input_keep_existing_semantics() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("root")).unwrap();
    child(&core, "child", "root", Lifetime::WithParent);
    let mut a = core.store().agent("child").unwrap();
    a.completion_pending = None;
    core.store().save(&a).unwrap();
    call(
        &mut core,
        "human",
        "stop",
        json!({"op":"stop","target":"root"}),
    )
    .unwrap();
    let parent = core.store().agent("root").unwrap();
    assert!(parent.paused);
    assert!(!core.store().agent("child").unwrap().expired);
    for state in [AgentState::Stopped, AgentState::Failed] {
        let mut a = parent.clone();
        a.state = state;
        a.control = None;
        core.store().save(&a).unwrap();
        core.ready_agents(0).unwrap();
        assert!(!core.store().agent("child").unwrap().expired);
    }
    let input = core.store().next_message("child").unwrap().unwrap();
    core.store()
        .message_state(&input, MessageState::Submitted)
        .unwrap();
    call(
        &mut core,
        "human",
        "end",
        json!({"op":"terminate","target":"root"}),
    )
    .unwrap();
    assert!(core.store().agent("child").unwrap().expired);
    assert_eq!(
        core.store().messages("child", None, 200).unwrap()["messages"][0]["state"],
        "submitted"
    );
}

#[test]
fn lifetimes_mid_cascade_failures_roll_back_root_descendants_events_and_dedup() {
    for complete in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut core = core(temp.path());
        core.store().save(&agent("root")).unwrap();
        child(&core, "child", "root", Lifetime::WithParent);
        child(&core, "grandchild", "child", Lifetime::WithParent);
        if complete {
            call(
                &mut core,
                "human",
                "complete",
                json!({"op":"complete","target":"root","body":"done"}),
            )
            .unwrap();
        }
        let db = rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_expiry BEFORE INSERT ON events WHEN NEW.agent='grandchild' AND NEW.kind='lifetime_expired' BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        let args = json!({"op":"terminate","target":"root"});
        let result = if complete {
            let mut a = core.store().agent("root").unwrap();
            core.atomic(|core| core.finish_task(&mut a))
        } else {
            call(&mut core, "human", "end", args.clone()).map(|_| ())
        };
        assert!(result.is_err());
        assert_eq!(
            core.store().agent("root").unwrap().task_outcome,
            TaskOutcome::Open
        );
        assert_eq!(
            core.store().events("root", 0, 200).unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            usize::from(complete)
        );
        for id in ["child", "grandchild"] {
            assert!(!core.store().agent(id).unwrap().expired);
            assert!(core.store().next_message(id).unwrap().is_some());
            assert!(expirations(&core, id).is_empty());
        }
        assert!(
            core.store()
                .prior("human", "end", &RequestEnvelope::new(args).fingerprint())
                .unwrap()
                .is_none()
        );
        db.execute_batch("DROP TRIGGER fail_expiry").unwrap();
        drop(db);
        drop(core);
        let restored = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
        assert!(!restored.agent("child").unwrap().expired);
    }
}

#[test]
fn lifetimes_corrupt_cycles_and_aliased_records_abort_without_partial_changes() {
    for alias in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut core = core(temp.path());
        let mut root = agent("root");
        root.parent_id = Some("child".into());
        root.lifetime = Lifetime::WithParent;
        core.store().save(&root).unwrap();
        child(&core, "child", "root", Lifetime::WithParent);
        if alias {
            let db = rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap();
            db.execute(
                "UPDATE agents SET data=json_set(data,'$.id','aliased') WHERE id='child'",
                [],
            )
            .unwrap();
        }
        assert_eq!(
            call(
                &mut core,
                "human",
                "end",
                json!({"op":"terminate","target":"root"})
            )
            .unwrap_err()
            .code(),
            "database_error"
        );
        assert_eq!(
            core.store().agent("root").unwrap().task_outcome,
            TaskOutcome::Open
        );
        assert!(!core.store().agent("child").unwrap().expired);
        assert!(expirations(&core, "child").is_empty());
    }
}

fn spawn_core(path: &Path) -> Core {
    let mut core = core(path);
    let profile: Profile =
        serde_json::from_value(json!({"provider":"codex","method":"subscription"})).unwrap();
    core.config_mut()
        .auth_profiles
        .insert("main".into(), profile.clone());
    core.state().register("main", &profile).unwrap();
    std::fs::write(path.join("role.md"), "instructions").unwrap();
    for (name, lifetime) in [
        ("free", Lifetime::Independent),
        ("bound", Lifetime::WithParent),
    ] {
        core.config_mut().roles.insert(
            name.into(),
            Role {
                options: std::collections::BTreeMap::default(),
                config: None,
                comb: None,
                mcp_servers: crate::config::McpServers::default(),
                auth: Some("main".into()),
                model: Some("mock".into()),
                effort: Some("high".into()),
                instructions: Some("role.md".into()),
                lifetime,
                provider: None,
                api_fallback: true,
                sandbox: crate::config::Sandbox::ReadOnly,
            },
        );
    }
    let mut a = agent("parent");
    a.cwd = path.to_owned();
    core.store().save(&a).unwrap();
    core
}

#[test]
fn sandbox_spawns_never_widen_agent_callers_and_batches_roll_back() {
    use crate::config::Sandbox::{FullAccess, ReadOnly, WorkspaceWrite};
    for parent_mode in [ReadOnly, WorkspaceWrite, FullAccess] {
        for child_mode in [ReadOnly, WorkspaceWrite, FullAccess] {
            for batch in [false, true] {
                let temp = crate::runtime::test_directory();
                let mut c = spawn_core(temp.path());
                let mut parent = c.store().agent("parent").unwrap();
                parent.sandbox = parent_mode;
                c.store().save(&parent).unwrap();
                let before = c.store().active_count().unwrap();
                let spawn = json!({"op":"spawn","role":"free","body":"work","cwd":temp.path(),"sandbox":child_mode,"request_id":"child"});
                let raw = if batch {
                    json!({"op":"batch","operations":[{"op":"send","target":"parent","body":"earlier","request_id":"message"},spawn]})
                } else {
                    spawn
                };
                let result = call(&mut c, "parent", "attempt", raw);
                let permitted = match parent_mode {
                    ReadOnly => child_mode == ReadOnly,
                    WorkspaceWrite => child_mode != FullAccess,
                    FullAccess => true,
                };
                assert_eq!(
                    result.is_ok(),
                    permitted,
                    "{parent_mode:?} -> {child_mode:?}, batch={batch}"
                );
                if permitted {
                    assert_eq!(c.store().active_count().unwrap(), before + 1);
                    let value = result.unwrap();
                    let id = if batch {
                        &value["results"][1]["agent_id"]
                    } else {
                        &value["agent_id"]
                    };
                    assert_eq!(
                        c.store().agent(id.as_str().unwrap()).unwrap().sandbox,
                        child_mode
                    );
                } else {
                    assert_eq!(result.unwrap_err().code(), "binding_conflict");
                    assert_eq!(c.store().active_count().unwrap(), before);
                    assert!(c.store().next_message("parent").unwrap().is_none());
                    assert!(!c.store().request_used("parent", "attempt").unwrap());
                }
            }
        }
    }
}

#[test]
fn sandbox_role_defaults_are_checked_and_human_spawns_may_choose_any_mode() {
    let temp = crate::runtime::test_directory();
    let mut c = spawn_core(temp.path());
    c.config_mut().roles.get_mut("free").unwrap().sandbox = crate::config::Sandbox::FullAccess;
    let args = json!({"op":"spawn","role":"free","body":"work","cwd":temp.path()});
    assert_eq!(
        call(&mut c, "parent", "wider-default", args.clone())
            .unwrap_err()
            .code(),
        "binding_conflict"
    );
    for mode in ["read-only", "workspace-write", "full-access"] {
        let mut request = args.clone();
        request["sandbox"] = json!(mode);
        let value = call(&mut c, "human", mode, request).unwrap();
        assert_eq!(
            c.store()
                .agent(value["agent_id"].as_str().unwrap())
                .unwrap()
                .sandbox
                .as_str(),
            mode
        );
    }
}

#[test]
fn spawn_widening_enabled_allows_explicit_and_role_modes_in_single_and_batch_spawns() {
    use crate::config::Sandbox::{FullAccess, ReadOnly, WorkspaceWrite};
    for (parent_mode, child_mode) in [
        (ReadOnly, WorkspaceWrite),
        (ReadOnly, FullAccess),
        (WorkspaceWrite, FullAccess),
    ] {
        for batch in [false, true] {
            let temp = crate::runtime::test_directory();
            let mut c = spawn_core(temp.path());
            c.config_mut().runtime.allow_spawn_widening = true;
            let mut parent = c.store().agent("parent").unwrap();
            parent.sandbox = parent_mode;
            c.store().save(&parent).unwrap();
            let spawn = json!({"op":"spawn","role":"free","body":"work","cwd":temp.path(),"sandbox":child_mode,"request_id":"child"});
            let raw = if batch {
                json!({"op":"batch","operations":[spawn]})
            } else {
                spawn
            };
            let result = call(&mut c, "parent", "explicit-widening", raw).unwrap();
            let id = if batch {
                &result["results"][0]["agent_id"]
            } else {
                &result["agent_id"]
            };
            let child = c.store().agent(id.as_str().unwrap()).unwrap();
            assert_eq!(child.sandbox, child_mode);
            assert_eq!(child.parent_id.as_deref(), Some("parent"));
            c.config_mut().roles.get_mut("free").unwrap().sandbox = child_mode;
            let role_spawn = json!({"op":"spawn","role":"free","body":"work","cwd":temp.path(),"request_id":"role-child"});
            let role_raw = if batch {
                json!({"op":"batch","operations":[role_spawn]})
            } else {
                role_spawn
            };
            let result = call(&mut c, "parent", "role-widening", role_raw).unwrap();
            let id = if batch {
                &result["results"][0]["agent_id"]
            } else {
                &result["agent_id"]
            };
            assert_eq!(
                c.store().agent(id.as_str().unwrap()).unwrap().sandbox,
                child_mode
            );
        }
    }
}

#[test]
fn lifetimes_spawn_overrides_and_strict_invalid_values_share_preflight_rules() {
    let temp = crate::runtime::test_directory();
    let mut core = spawn_core(temp.path());
    for (caller, role, value, expected) in [
        ("human", "bound", Some("independent"), Lifetime::Independent),
        ("parent", "free", Some("with_parent"), Lifetime::WithParent),
        (
            "parent",
            "bound",
            Some("independent"),
            Lifetime::Independent,
        ),
        ("parent", "bound", None, Lifetime::WithParent),
        ("human", "free", None, Lifetime::Independent),
    ] {
        let mut args = json!({"op":"spawn","role":role,"body":"work","cwd":temp.path()});
        if let Some(value) = value {
            args["lifetime"] = json!(value);
        }
        let accepted = call(&mut core, caller, &uuid::Uuid::new_v4().to_string(), args).unwrap();
        assert_eq!(
            core.store()
                .agent(accepted["agent_id"].as_str().unwrap())
                .unwrap()
                .lifetime,
            expected
        );
    }
    for role in ["free", "bound"] {
        let args = json!({"op":"spawn","role":role,"body":"work","cwd":temp.path(),"lifetime":"with_parent"});
        assert_eq!(
            call(&mut core, "human", role, args).unwrap_err().code(),
            "parent_required"
        );
    }
    for value in [
        json!("persistent"),
        json!("until_completed"),
        json!("until_parent_completed"),
        json!("bad"),
        json!(0),
        json!(null),
    ] {
        let args =
            json!({"op":"spawn","role":"free","body":"work","cwd":temp.path(),"lifetime":value});
        assert_eq!(
            call(&mut core, "parent", "invalid", args)
                .unwrap_err()
                .code(),
            "invalid_value"
        );
    }
    let envelope = RequestEnvelope::new(
        json!({"op":"spawn","role":"free","body":"work","lifetime":"with_parent"}),
    );
    let captured = crate::runtime::core::preflight::RequestInput::capture(
        &core,
        &crate::runtime::hub::Caller::test_agent("parent"),
        &envelope,
    );
    call(
        &mut core,
        "human",
        "end-parent",
        json!({"op":"terminate","target":"parent"}),
    )
    .unwrap();
    let facts = captured.resolve();
    assert_error(
        core.dispatch(
            &crate::runtime::hub::Caller::test_agent("parent"),
            "raced-spawn",
            &envelope,
            &facts,
        ),
        completed(),
    );
}

#[test]
fn spawn_preflight_preserves_a_failed_parent_lookup_even_with_explicit_cwd() {
    let temp = crate::runtime::test_directory();
    let core = spawn_core(temp.path());
    let sql = rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap();
    // Valid JSON but an invalid saved agent: this is an actual database decode
    // failure, not a missing parent, and explicit cwd must not mask it.
    sql.execute(
        "UPDATE agents SET data=json_remove(data,'$.created_at') WHERE id='parent'",
        [],
    )
    .unwrap();
    assert_eq!(
        core.store().agent("parent").unwrap_err().code(),
        "database_error"
    );
    let envelope = RequestEnvelope::new(json!({
        "op":"spawn", "role":"free", "body":"work", "cwd":temp.path()
    }));
    let facts = crate::runtime::core::preflight::RequestInput::capture(
        &core,
        &crate::runtime::hub::Caller::test_agent("parent"),
        &envelope,
    )
    .resolve();
    let resolved = &facts.spawn(&envelope).unwrap().resolved;
    assert_eq!(
        resolved.as_ref().err().map(Error::code),
        Some("database_error")
    );
}

#[test]
fn lifetimes_batch_self_termination_preserves_order_and_outer_atomicity() {
    for terminate_first in [false, true] {
        let temp = crate::runtime::test_directory();
        let mut core = spawn_core(temp.path());
        child(&core, "existing", "parent", Lifetime::WithParent);
        let spawn = json!({"op":"spawn","role":"free","body":"work","lifetime":"with_parent","request_id":"spawn"});
        let end = json!({"op":"terminate","target":"parent","request_id":"end"});
        let operations = if terminate_first {
            vec![end, spawn]
        } else {
            vec![spawn, end]
        };
        let args = json!({"op":"batch","operations":operations});
        let result = call(&mut core, "parent", "batch", args.clone());
        if terminate_first {
            assert_error(result, completed());
            assert_eq!(
                core.store().agent("parent").unwrap().task_outcome,
                TaskOutcome::Open
            );
            assert!(!core.store().agent("existing").unwrap().expired);
            assert!(expirations(&core, "existing").is_empty());
        } else {
            let accepted = result.unwrap();
            let spawned = accepted["results"][0]["agent_id"].as_str().unwrap();
            assert!(core.store().agent(spawned).unwrap().expired);
            assert_eq!(call(&mut core, "parent", "batch", args).unwrap(), accepted);
            assert_eq!(expirations(&core, spawned).len(), 1);
        }
    }
}

#[test]
fn lifetimes_tool_catalog_and_config_accept_only_the_two_current_values() {
    assert_eq!(Lifetime::default(), Lifetime::Independent);
    for value in [
        "independent",
        "with_parent",
        "persistent",
        "until_completed",
        "until_parent_completed",
    ] {
        let valid = matches!(value, "independent" | "with_parent");
        assert_eq!(
            serde_json::from_value::<Lifetime>(json!(value)).is_ok(),
            valid
        );
        let args = json!({"role":"free","body":"work","request_id":"spawn","lifetime":value});
        assert_eq!(validate_tool("hive_spawn", &args).is_ok(), valid);
        let mut operation = args;
        operation["op"] = json!("spawn");
        assert_eq!(
            validate_tool(
                "hive_batch",
                &json!({"request_id":"batch","operations":[operation]})
            )
            .is_ok(),
            valid
        );
    }
}
