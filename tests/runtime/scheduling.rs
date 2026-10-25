use super::*;

#[test]
fn clean_linger_expires_without_another_agent_or_shutdown() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({"max_loaded":4,"max_providers":1,
        "sessions_per_codex_process":4,"max_starting":1,"max_active_turns":4,
        "linger_seconds":3}),
        false,
    );
    f.start();
    let id = f.spawn("complete");
    f.wait(&id, |a| a["state"] == "stopped");
    let resources = f.call(json!({"op":"ping"}))["runtime_resources"].clone();
    assert_eq!(resources["lingering_shards"], 1);
    assert_eq!(resources["resident_session_slots"], 1);
    let resources = wait_resources(
        &f,
        Duration::from_secs(10),
        support::wait::POLL_INTERVAL,
        |resources| resources["reserved_shards"] == 0,
    );
    assert_eq!(resources["resident_session_slots"], 0);
    f.shutdown();
}

#[test]
fn default_sharing_reuses_sixteen_conversations_before_retiring_the_process() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({
            "max_loaded":16, "max_providers":1,
            "max_starting":1, "max_active_turns":4, "linger_seconds":30,
        }),
        false,
    );
    let trace = f.tmp.path().join("methods.log");
    f.set_scenario(Provider::Codex, json!({"trace":trace}));
    f.start();
    for n in 1..=17 {
        let id = f.spawn("complete");
        f.wait(&id, |a| {
            a["state"] == "stopped" && a["task_outcome"] == "completed"
        });
        let methods = fs::read_to_string(&trace).unwrap();
        // One catalog process plus one runtime process for the first sixteen;
        // the seventeenth conversation requires a new process, never a recycled slot.
        assert_eq!(
            methods.lines().filter(|m| *m == "initialize").count(),
            if n <= 16 { 2 } else { 3 }
        );
    }
    f.shutdown();
}

#[test]
fn wrong_auth_used_linger_yields_capacity_before_cohort_minimum_age() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({
            "max_loaded":4, "max_providers":1, "sessions_per_codex_process":4,
            "max_starting":1, "max_active_turns":4,
            "linger_seconds":3600, "cohort_min_seconds":3600,
        }),
        true,
    );
    f.start();
    let id = f.spawn("complete");
    f.wait(&id, |a| a["state"] == "stopped");
    assert_eq!(
        f.call(json!({"op":"ping"}))["runtime_resources"]["lingering_shards"],
        1
    );
    let mut response = Value::Null;
    let other = support::wait::try_wait_until(
        Duration::from_secs(10),
        support::wait::POLL_INTERVAL,
        || {
            response = f.call(json!({"op":"spawn", "role":"implementor", "auth":"other",
            "cwd":f.tmp.path(), "body":"complete"}));
            if response["agent_id"].is_string() {
                return Some(response.clone());
            }
            assert_eq!(response["error"]["code"], "agent_busy", "{response}");
            None
        },
    )
    .unwrap_or_else(|| panic!("{response}"));
    f.wait(other["agent_id"].as_str().unwrap(), |a| {
        a["state"] == "stopped" && a["task_outcome"] == "completed"
    });
    f.shutdown();
}

#[test]
fn burst_roots_complete_seed_only_delegation_under_residency_pressure() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({
            "max_loaded":8,
            "max_providers":2,
            "sessions_per_codex_process":4,
            "max_starting":4,
            "max_active_turns":8,
            "idle_seconds":0,
            "cohort_min_seconds":1,
        }),
        false,
    );
    f.start();
    let operations: Vec<_> = (0..16)
        .map(|i| {
            json!({
                "op":"spawn", "role":"implementor", "cwd":f.tmp.path(),
                "body":"delegate", "request_id":format!("root-{i}"),
            })
        })
        .collect();
    let result = f.call(json!({"op":"batch","operations":operations}));
    let roots = result["results"].as_array().unwrap();
    assert_eq!(roots.len(), 16);
    for root in roots {
        f.wait_timeout(
            root["agent_id"].as_str().unwrap(),
            Duration::from_secs(60),
            |a| a["task_outcome"] == "completed" && a["state"] == "stopped",
        );
    }
    f.shutdown();
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let count = |sql| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM agents"), 32);
    assert_eq!(
        count(
            "SELECT count(*) FROM agents WHERE parent_id IS NOT NULL AND expired=1 AND state='stopped'"
        ),
        16
    );
    assert_eq!(
        count("SELECT count(*) FROM events WHERE kind='execution_failed'"),
        0
    );
    assert_eq!(
        count(
            "SELECT count(*) FROM messages WHERE body='review one' AND startup=1 AND state='completed'"
        ),
        16
    );
    assert_eq!(
        count("SELECT count(*) FROM messages WHERE body='NO_FINDINGS' AND state='completed'"),
        16
    );
    assert_eq!(
        count("SELECT count(*) FROM messages WHERE state='accepted'"),
        0
    );
    assert_eq!(count("SELECT active FROM runtime_counts"), 0);
}

#[test]
fn retained_terminal_history_does_not_exhaust_nonterminal_capacity() {
    let mut f = Harness::new();
    // Exercise a fixed capacity boundary independently of product defaults.
    configure_runtime(&f, json!({"max_agents":128,"max_loaded":128}), false);
    f.start();
    let id = f.spawn("complete");
    let template = f.wait(&id, |a| {
        a["state"] == "stopped" && a["task_outcome"] == "completed"
    });
    f.shutdown();
    seed_capacity_agents(&f, &id, &template, 130, false);
    f.start();
    let fresh = f.spawn("complete");
    f.wait(&fresh, |a| {
        a["state"] == "stopped" && a["task_outcome"] == "completed"
    });
    f.shutdown();
    seed_capacity_agents(&f, &id, &template, 128, true);
    f.start();
    assert_eq!(
        f.call(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"idle"}))["error"]
            ["code"],
        "agent_limit"
    );
    f.shutdown();
}

#[test]
fn warm_spares_have_no_threads_expire_and_do_not_respawn_without_demand() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({
            "max_loaded":4,
            "max_providers":4,
            "max_starting":2,
            "max_active_turns":4,
            "warm_providers":2,
            // Leave room for concurrent provider startup on CI before observing
            // the spare. Its lifetime starts at reservation, not at readiness.
            "warm_idle_seconds":10,
        }),
        false,
    );
    f.start();
    let id = f.spawn("idle");
    let resources = wait_resources(
        &f,
        Duration::from_secs(10),
        Duration::from_millis(10),
        |resources| {
            assert!(resources["reserved_shards"].as_u64().unwrap() <= 4);
            assert!(resources["starting_shards"].as_u64().unwrap() <= 2);
            resources["warm_shards"].as_u64().unwrap() >= 1
        },
    );
    assert_eq!(resources["resident_session_slots"], 1);
    f.wait(&id, |a| a["state"] == "idle");
    let resources = wait_resources(
        &f,
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        |resources| resources["reserved_shards"] == 1,
    );
    assert_eq!(resources["warm_shards"], 0);
    assert_eq!(resources["resident_session_slots"], 1);
    // Expired spares must stay gone without new demand, even across scheduler ticks.
    let quiet = Instant::now();
    while quiet.elapsed() < Duration::from_secs(1) {
        let resources = f.call(json!({"op":"ping"}))["runtime_resources"].clone();
        assert_eq!(resources["reserved_shards"], 1);
        assert_eq!(resources["resident_session_slots"], 1);
        std::thread::sleep(Duration::from_millis(20));
    }
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM events WHERE kind='session_ready'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    f.shutdown();
}
#[test]
fn wrong_auth_spare_yields_capacity_before_cohort_minimum_age() {
    let mut f = Harness::new();
    configure_runtime(
        &f,
        json!({
            "max_loaded":2,
            "max_providers":2,
            "max_starting":2,
            "max_active_turns":2,
            "warm_providers":1,
            "sessions_per_codex_process":1,
            "cohort_min_seconds":3600,
        }),
        true,
    );
    f.start();
    let id = f.spawn("complete");
    f.wait(&id, |a| a["state"] == "stopped");
    let other = f.call(
        json!({"op":"spawn","role":"implementor","auth":"other","cwd":f.tmp.path(),"body":"idle"}),
    );
    f.wait(other["agent_id"].as_str().unwrap(), |a| {
        a["state"] == "idle"
    });
    let next = f.call(json!({
        "op": "spawn",
        "role": "implementor",
        "auth": "other",
        "cwd": f.tmp.path(),
        "body": "complete"
    }));
    f.wait(next["agent_id"].as_str().unwrap(), |a| {
        a["state"] == "stopped" && a["task_outcome"] == "completed"
    });
    f.shutdown();
}

fn seed_capacity_agents(f: &Harness, id: &str, template: &Value, count: usize, resumable: bool) {
    let db = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(agents)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .filter(|c| !["id", "listing_seq"].contains(&c.as_str()))
        .collect();
    let selection = columns
        .iter()
        .map(|column| match column.as_str() {
            "data" => "json_set(data,'$.id',?1)".to_owned(),
            "task_outcome" => "?3".to_owned(),
            "paused" => "?4".to_owned(),
            // Resumable agents have no end time, unlike the completed template.
            "ended_at" if resumable => "NULL".to_owned(),
            _ => column.clone(),
        })
        .collect::<Vec<_>>()
        .join(",");
    let statement = format!(
        "INSERT INTO agents(id,{}) SELECT ?1,{selection} FROM agents WHERE id=?2",
        columns.join(",")
    );
    for _ in 0..count {
        let mut agent = template.clone();
        agent["id"] = json!(Uuid::new_v4().to_string());
        if resumable {
            agent["task_outcome"] = json!("open");
            agent["paused"] = json!(true);
        }
        db.execute(
            &statement,
            rusqlite::params![
                agent["id"].as_str().unwrap(),
                id,
                agent["task_outcome"].as_str().unwrap(),
                agent["paused"].as_bool().unwrap()
            ],
        )
        .unwrap();
    }
}

fn wait_resources(
    f: &Harness,
    timeout: Duration,
    interval: Duration,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let mut resources = Value::Null;
    support::wait::try_wait_until(timeout, interval, || {
        resources = f.ok(json!({"op":"ping"}))["runtime_resources"].clone();
        predicate(&resources).then(|| resources.clone())
    })
    .unwrap_or_else(|| panic!("runtime resources did not reach expected state: {resources}"))
}
