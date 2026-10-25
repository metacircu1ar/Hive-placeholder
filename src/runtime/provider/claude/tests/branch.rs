use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

fn write_history(path: &std::path::Path, rows: &[Value]) {
    fs::write(
        path,
        rows.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn prepared() -> (Fixture, PathBuf, Vec<Value>, String) {
    let source = Uuid::new_v4().to_string();
    let cutoff = Uuid::new_v4().to_string();
    let tail = Uuid::new_v4().to_string();
    let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
        config.history = History::Fork {
            source: source.clone(),
            cutoff: cutoff.clone(),
        };
    });
    let home = f.dir.path().join("native");
    crate::state::directory(&home).unwrap();
    crate::state::directory(&home.join("projects")).unwrap();
    let project = home.join("projects/main");
    crate::state::directory(&project).unwrap();
    let rows = vec![
        json!({"type":"user","uuid":cutoff,"parentUuid":null,"message":{"role":"user","content":"source fact"}}),
        json!({"type":"assistant","uuid":tail,"parentUuid":cutoff,"message":{"role":"assistant","content":"settled"}}),
    ];
    write_history(&project.join(format!("{source}.jsonl")), &rows);
    f.codec.prepare_branch(&home).unwrap();
    let input = f
        .codec
        .send(outgoing::request(3, outgoing::turn_input(&f.id, "task")))
        .unwrap();
    let turn = input[0]["uuid"].as_str().unwrap().to_owned();
    f.codec.receive(json!({"type":"system","subtype":"init","session_id":f.id,"model":"claude-sonnet-5","permissionMode":"default"})).unwrap();
    f.codec
        .receive(json!({"type":"user","uuid":turn,"session_id":f.id,"isReplay":true}))
        .unwrap();
    assert!(f.codec.pop().is_none());
    (f, project, rows, turn)
}
fn copied(rows: &[Value], turn: &str) -> Vec<Value> {
    let mut child = rows.to_vec();
    child.push(
        json!({"type":"user","uuid":turn,"parentUuid":rows.last().unwrap()["uuid"],
        "message":{"role":"user","content":"task"}}),
    );
    child
}
#[test]
fn fork_withholds_permissions_and_output_until_history_and_binding_are_verified() {
    let (mut f, project, rows, turn) = prepared();
    let permission = json!({"type":"control_request","request_id":"permission","request":{
        "subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"/tmp/read-only"}}});
    assert!(f.codec.receive(permission).unwrap().is_empty());
    assert!(f.codec.poll_branch().unwrap().is_empty());
    assert!(f.codec.pop().is_none());
    assert!(f.codec.commit_branch(&turn).is_err());
    write_history(
        &project.join(format!("{}.jsonl", f.id)),
        &copied(&rows, &turn),
    );
    assert!(f.codec.poll_branch().unwrap().is_empty());
    assert_eq!(f.codec.pop().unwrap()["id"], 3);
    assert_eq!(f.codec.pop().unwrap()["method"], "turn/started");
    f.assistant(
        Some("answer"),
        "claude-sonnet-5",
        json!([{"type":"text","text":"withheld"}]),
    );
    assert!(f.codec.pop().is_none());
    assert!(f.codec.poll_branch().unwrap().is_empty());
    f.codec.commit_branch(&turn).unwrap();
    let replies = f.codec.poll_branch().unwrap();
    assert_eq!(replies[0]["response"]["response"]["behavior"], "allow");
}
#[test]
fn malformed_copied_chain_never_authorizes_native_hooks_permissions_or_mcp() {
    for defect in ["disconnected", "changed", "different_input"] {
        let (mut f, project, rows, turn) = prepared();
        for request in [
            json!({"subtype":"can_use_tool","tool_name":"Read","input":{}}),
            json!({"subtype":"hook_callback","callback_id":"hive-guard","input":{}}),
            json!({"subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}}),
        ] {
            assert!(
                f.codec
                    .receive(
                        json!({"type":"control_request","request_id":"held","request":request})
                    )
                    .unwrap()
                    .is_empty()
            );
        }
        let mut child = copied(&rows, &turn);
        match defect {
            "disconnected" => child[2]["parentUuid"] = Value::Null,
            "changed" => child[0]["message"]["content"] = json!("different history"),
            _ => child[2]["uuid"] = json!(Uuid::new_v4().to_string()),
        }
        write_history(&project.join(format!("{}.jsonl", f.id)), &child);
        assert!(f.codec.poll_branch().is_err(), "{defect}");
        assert!(f.codec.pop().is_none());
        assert!(f.codec.commit_branch(&turn).is_err());
        assert!(!f.codec.preinput_rejected());
    }
}

#[test]
fn timeout_or_interrupt_never_releases_unverified_tools_or_reclassifies_input() {
    for interrupted in [false, true] {
        let (mut f, _project, _rows, turn) = prepared();
        f.codec
            .receive(
                json!({"type":"control_request","request_id":"held","request":{
            "subtype":"can_use_tool","tool_name":"Read","input":{}}}),
            )
            .unwrap();
        if interrupted {
            let request = f
                .codec
                .send(outgoing::request(4, outgoing::interrupt(&f.id, &turn)))
                .unwrap();
            assert_eq!(request[0]["request"]["subtype"], "interrupt");
        }
        let now = std::time::Instant::now()
            + if interrupted {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_secs(16)
            };
        assert!(f.codec.poll_branch_at(now).is_err());
        assert!(f.codec.pop().is_none());
        assert!(f.codec.commit_branch(&turn).is_err());
        assert!(!f.codec.preinput_rejected());
    }
}

#[test]
fn committed_branch_drains_held_requests_before_new_arrivals() {
    let (mut f, project, rows, turn) = prepared();
    let permission = |i: usize| {
        json!({"type":"control_request","request_id":format!("request-{i}"),"request":{
        "subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"/tmp/read-only"}}})
    };
    for i in 0..40 {
        assert!(f.codec.receive(permission(i)).unwrap().is_empty());
    }
    write_history(
        &project.join(format!("{}.jsonl", f.id)),
        &copied(&rows, &turn),
    );
    f.codec.poll_branch().unwrap();
    f.codec.commit_branch(&turn).unwrap();
    let first = f.codec.poll_branch().unwrap();
    assert_eq!(first.len(), 32);
    assert!(f.codec.receive(permission(40)).unwrap().is_empty());
    let rest = f.codec.poll_branch().unwrap();
    assert_eq!(rest.len(), 9);
    for (i, reply) in first.into_iter().chain(rest).enumerate() {
        assert_eq!(reply["response"]["request_id"], format!("request-{i}"));
    }
}
