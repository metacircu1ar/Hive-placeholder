use super::*;
use std::os::unix::process::ExitStatusExt;

fn observed_events(path: &std::path::Path) -> Vec<Value> {
    let output = fs::read_to_string(path).unwrap();
    // A concurrent write can leave the last page incomplete in this snapshot.
    let mut lines: Vec<_> = output.split('\n').collect();
    lines.pop();
    lines
        .into_iter()
        .flat_map(|line| {
            let page: Value = serde_json::from_str(line).unwrap();
            assert_eq!(page["history_expired"], false);
            page["events"].as_array().unwrap().clone()
        })
        .collect()
}

fn observe_turns(path: &std::path::Path, count: usize) -> Vec<Value> {
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "followed turn outcomes",
        || {
            let events = observed_events(path);
            (events
                .iter()
                .filter(|event| event["type"] == "turn_finished")
                .count()
                >= count)
                .then_some(events)
        },
    )
}

#[test]
fn follows_live_turns_and_interrupting_the_viewer_leaves_the_agent_usable() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("reply:first");
    let path = f.tmp.path().join("follow.jsonl");
    let mut viewer = support::ChildGuard::new(
        support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["read", &id, "--follow", "--limit", "1"])
            .stdout(fs::File::create(&path).unwrap())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    observe_turns(&path, 1);
    let send = f.ok(json!({"op":"send","target":id,"body":"reply:second"}));
    super::lifecycle::wait_completed_input(&f, &id, send["message_id"].as_str().unwrap());
    let events = observe_turns(&path, 2);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0]["seq"].as_u64() < pair[1]["seq"].as_u64())
    );
    assert!(events.iter().all(|event| event["agent_id"] == id));
    assert!(events.iter().any(
        |event| event["type"] == "message_accepted" && event["data"]["body"] == "reply:second"
    ));
    assert!(viewer.try_wait().unwrap().is_none());
    let thread = f.wait(&id, |agent| agent["state"] == "idle")["thread_id"].clone();
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&viewer),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let exit = support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "follow cancellation",
        || viewer.try_wait().unwrap(),
    );
    assert_eq!(exit.signal(), Some(libc::SIGINT));
    let send = f.ok(json!({"op":"send","target":id,"body":"reply:after-viewer"}));
    super::lifecycle::wait_completed_input(&f, &id, send["message_id"].as_str().unwrap());
    let agent = f.wait(&id, |agent| agent["state"] == "idle");
    assert_eq!(agent["thread_id"], thread);
    assert_eq!(agent["task_outcome"], "open");
    assert_eq!(agent["paused"], false);
    f.shutdown();
}
