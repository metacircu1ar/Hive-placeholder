use super::*;
use control_inputs::assert_input;

fn configured(c: &mut Core) {
    control_inputs::configure_spawn(c);
    c.config_mut().roles.get_mut("test").unwrap().instructions = None;
}

fn spawn(cwd: &Path) -> Value {
    json!({"op":"spawn","role":"test","cwd":cwd,"body":"work"})
}

#[test]
fn explicit_directories_report_fields_without_inheriting_over_bad_values() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    c.store().save(&agent("parent")).unwrap();
    for value in [
        json!(""),
        json!("relative"),
        json!("/private-canary\0"),
        json!(temp.path().join("private-canary-missing")),
        json!(temp.path().join("runtime.sqlite")),
    ] {
        let mut raw = spawn(temp.path());
        raw["cwd"] = value;
        assert_input(
            call(&mut c, "parent", "bad-cwd", raw),
            "invalid_path",
            "/cwd",
            "spawn",
        );
    }
    let raw = json!({"op":"spawn","role":"test","body":"work"});
    assert_input(
        call(&mut c, "human", "missing", raw),
        "missing_argument",
        "/cwd",
        "spawn",
    );
    assert_eq!(c.store().active_count().unwrap(), 1);
}

#[test]
fn inherited_directory_failures_remain_operational() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    let mut parent = agent("parent");
    parent.cwd = temp.path().join("private-canary-missing");
    c.store().save(&parent).unwrap();
    let error = call(
        &mut c,
        "parent",
        "inherited",
        json!({"op":"spawn","role":"test","body":"work"}),
    )
    .unwrap_err();
    assert_eq!(error.code(), "invalid_cwd");
    assert_eq!(error.io_raw_os_error(), Some(libc::ENOENT));
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private-canary")
    );
}

#[test]
fn symlink_cwd_is_canonicalized_and_accepted_replay_does_not_resolve_it_again() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    let alias = temp.path().join("workspace-link");
    std::os::unix::fs::symlink(temp.path(), &alias).unwrap();
    let raw = spawn(&alias);
    let accepted = call(&mut c, "human", "stable", raw.clone()).unwrap();
    let saved = c
        .store()
        .agent(accepted["agent_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.cwd, temp.path().canonicalize().unwrap());
    std::fs::remove_file(alias).unwrap();
    assert_eq!(call(&mut c, "human", "stable", raw).unwrap(), accepted);
}

#[test]
fn explicit_instruction_byte_limit_precedes_config_reads_and_accepts_empty() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    c.config_mut().context = vec!["absent.md".into()];
    let mut raw = spawn(temp.path());
    raw["instructions"] = json!("é".repeat(32769));
    assert_input(
        call(&mut c, "human", "oversized", raw.clone()),
        "out_of_range",
        "/instructions",
        "spawn",
    );
    c.config_mut().context.clear();
    for (id, text) in [("exact", "é".repeat(32768)), ("empty", String::new())] {
        raw["instructions"] = json!(text);
        let accepted = call(&mut c, "human", id, raw.clone()).unwrap();
        assert_eq!(
            c.store()
                .instructions(accepted["agent_id"].as_str().unwrap())
                .unwrap(),
            text
        );
    }
}

fn fill_context(c: &mut Core, path: &Path) {
    std::fs::write(path.join("context.md"), "x".repeat(65534)).unwrap();
    c.config_mut().context = vec!["context.md".into(); 4];
}

#[test]
fn aggregate_instructions_attribute_only_explicit_overflow_to_the_request() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    fill_context(&mut c, temp.path());
    let mut raw = spawn(temp.path());
    raw["instructions"] = json!("private-canary");
    assert_input(
        call(&mut c, "human", "explicit", raw.clone()),
        "out_of_range",
        "/instructions",
        "spawn",
    );
    raw["instructions"] = json!("");
    let accepted = call(&mut c, "human", "exact", raw.clone()).unwrap();
    assert_eq!(
        c.store()
            .instructions(accepted["agent_id"].as_str().unwrap())
            .unwrap()
            .len(),
        262_144
    );
    c.config_mut().context.push("context.md".into());
    assert_eq!(
        call(&mut c, "human", "config", raw.clone())
            .unwrap_err()
            .code(),
        "invalid_request"
    );
    c.config_mut().context.push("absent.md".into());
    assert_eq!(
        call(&mut c, "human", "first-file-failure", raw)
            .unwrap_err()
            .code(),
        "instructions_unavailable"
    );
}

#[test]
fn context_rejection_inside_batch_retains_index_and_rolls_back_earlier_spawns() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    let mut first = spawn(temp.path());
    first["request_id"] = json!("first");
    let mut second = first.clone();
    second["request_id"] = json!("second");
    second["cwd"] = json!("private-canary-relative");
    assert_input(
        call(
            &mut c,
            "human",
            "batch",
            json!({"op":"batch","operations":[first,second]}),
        ),
        "invalid_path",
        "/operations/1/cwd",
        "batch",
    );
    assert_eq!(c.store().active_count().unwrap(), 0);
}
