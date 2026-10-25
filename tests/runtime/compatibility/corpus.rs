//! Synthetic release corpus: real mock-produced state, upgraded in place.
//! Copying the tree would invalidate inode-bound ownership evidence.
use super::*;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, path::Path};

fn image(root: &Path) -> BTreeMap<PathBuf, (u64, u32, Vec<u8>)> {
    let mut image = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.path().symlink_metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                image.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    (
                        metadata.ino(),
                        metadata.mode(),
                        fs::read(entry.path()).unwrap(),
                    ),
                );
            } else {
                panic!(
                    "quiescent corpus contains a non-regular entry: {}",
                    entry.path().display()
                );
            }
        }
    }
    image
}

#[test]
fn synthetic_release_corpus_keeps_bindings_artifacts_and_delivers_queued_input_once() {
    let mut f = Harness::new();
    f.start();
    let large = "x".repeat(32768);
    let source = f.spawn(&format!("reply:{large}"));
    let before_source = f.wait(&source, |a| {
        a["state"] == "idle" && !a["last_finished_turn"].is_null()
    });
    let events = f.events(&source);
    let artifact = events
        .events
        .iter()
        .find_map(|e| e.data.get("artifact"))
        .unwrap();
    let output_request = json!({"op":"output","id":artifact["id"],"offset":0,"limit":65536});
    let output = f.ok(output_request.clone());
    assert!(output.to_string().contains(&large));
    let spawned = f.ok(json!({"op":"spawn","from_agent":source,"body":"reply:branch seed"}));
    let child = spawned["agent_id"].as_str().unwrap().to_owned();
    let before_child = f.wait(&child, |a| {
        a["state"] == "idle" && a["branch_phase"] == "bound" && !a["last_finished_turn"].is_null()
    });
    assert_ne!(before_source["thread_id"], before_child["thread_id"]);
    f.ok(json!({"op":"stop","target":child}));
    f.wait(&child, |a| a["state"] == "stopped" && a["paused"] == true);
    let attachment = f.tmp.path().join("evidence.txt");
    fs::write(
        &attachment,
        "Private synthetic attachment, retained after source deletion.",
    )
    .unwrap();
    let queued: Vec<_> = (0..3).map(|i| {
        let id = format!("corpus-message-{i}");
        let args = json!({"op":"send","target":child,"body":format!("reply:queued-{i}"),"attachments":[attachment]});
        let response = f.call_id(&id, args.clone());
        assert!(response["error"].is_null(), "{response}");
        (id, args, response)
    }).collect();
    fs::remove_file(attachment).unwrap();
    f.shutdown();

    let state = State::new(f.root.clone()).unwrap();
    let before = image(&f.root);
    for dry_run in [true, false] {
        let report = state.upgrade(dry_run).unwrap();
        assert_eq!(report.status, "current");
        assert!(report.steps.is_empty() && report.backups.is_empty());
        assert_eq!(image(&f.root), before, "upgrade changed current corpus");
    }
    f.start();
    let restored = f.status(&child);
    for field in [
        "thread_id",
        "auth_generation",
        "branched_from_agent_id",
        "branch_phase",
    ] {
        assert!(!before_child[field].is_null(), "missing {field}");
        assert_eq!(restored[field], before_child[field], "lost {field}");
    }
    assert_eq!(f.ok(output_request), output);
    for (id, args, response) in &queued {
        assert_eq!(f.call_id(id, args.clone()), *response);
    }
    f.ok(json!({"op":"resume","target":child}));
    for (_, _, response) in &queued {
        let row = super::super::steering::delivery(&f, &child, response, "completed");
        assert_eq!(row["attachments"].as_array().unwrap().len(), 1);
    }
    f.wait(&child, |a| a["state"] == "idle");
    delivered_once(&f, &queued, &before_child["thread_id"]);
    assert_eq!(f.status(&source)["thread_id"], before_source["thread_id"]);
    f.shutdown();
}

#[test]
fn newer_corpus_is_refused_without_changing_any_owned_file() {
    let mut f = Harness::new();
    f.start();
    let agent = f.spawn("reply:retained");
    f.wait(&agent, |a| {
        a["state"] == "idle" && !a["last_finished_turn"].is_null()
    });
    f.shutdown();
    fs::write(f.root.join("format"), b"{\"format\":4294967295}\n").unwrap();
    let before = image(&f.root);
    let state = State::new(f.root.clone()).unwrap();
    for dry_run in [true, false] {
        assert_eq!(state.upgrade(dry_run).err().unwrap().code(), "newer_format");
        assert_eq!(image(&f.root), before);
    }
}

fn delivered_once(f: &Harness, queued: &[(String, Value, Value)], thread: &Value) {
    let history: Vec<Value> = fs::read_to_string(f.provider_home.join("inputs.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (_, args, _) in queued {
        let matching: Vec<_> = history
            .iter()
            .filter(|frame| frame.to_string().contains(args["body"].as_str().unwrap()))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "queued request duplicated or lost: {args}"
        );
        assert_eq!(matching[0]["thread"], *thread);
    }
}
