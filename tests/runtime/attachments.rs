use super::steering::delivery;
use super::*;
use std::io::Cursor;

fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
fn inputs(f: &Harness, provider: Provider) -> Vec<Value> {
    fs::read_to_string(f.provider_home(provider).join("inputs.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn assert_image(frame: &Value, provider: Provider, bytes: &[u8]) {
    let input = &frame["native_input"];
    assert_eq!(input.as_array().unwrap().len(), 2, "{frame}");
    assert_eq!(input[0]["type"], "text");
    assert!(
        input[0]["text"]
            .as_str()
            .unwrap()
            .contains("Hive attachment references")
    );
    if provider == Provider::Codex {
        assert_eq!(input[1]["type"], "localImage");
        assert_eq!(fs::read(input[1]["path"].as_str().unwrap()).unwrap(), bytes);
        assert_eq!(input[1].as_object().unwrap().len(), 2);
    } else {
        use base64::Engine;
        assert_eq!(input[1]["type"], "image");
        assert_eq!(input[1]["source"]["type"], "base64");
        assert_eq!(input[1]["source"]["media_type"], "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(input[1]["source"]["data"].as_str().unwrap())
                .unwrap(),
            bytes
        );
    }
}

provider_matrix!(
    attachments_native_spawn_delayed_replay_and_redirect,
    native_modes
);
fn native_modes(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    let source = f.tmp.path().join("screenshot.log");
    let bytes = png();
    fs::write(&source, &bytes).unwrap();
    f.start();
    let role = if provider == Provider::Claude {
        "claude"
    } else {
        "implementor"
    };
    let spawned=f.ok(json!({"op":"spawn","role":role,"cwd":f.tmp.path(),"body":"reply:first","attachments":[source]}));
    let id = spawned["agent_id"].as_str().unwrap().to_owned();
    delivery(&f, &id, &spawned, "completed");
    assert_image(&inputs(&f, provider)[0], provider, &bytes);
    let delayed = json!({"op":"send","target":id,"body":"reply:delayed","delay_seconds":2,"attachments":[source]});
    let accepted = f.call_id("delayed-image", delayed.clone());
    assert!(accepted["error"].is_null(), "{accepted}");
    fs::remove_file(&source).unwrap();
    assert_eq!(f.call_id("delayed-image", delayed), accepted);
    let row = delivery(&f, &id, &accepted, "completed");
    assert_eq!(row["attachments"][0]["delivery"]["kind"], "native_image");
    let status = f.ok(json!({"op":"status","target":id}));
    assert_eq!(
        status["latest_attachments"]["message_id"],
        accepted["message_id"]
    );
    assert_eq!(
        status["latest_attachments"]["attachments"],
        row["attachments"]
    );
    assert!(
        f.read(&id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "turn_started"
                && e["data"]["attachments"][0]["delivery"]["kind"] == "native_image")
    );
    assert_image(&inputs(&f, provider)[1], provider, &bytes);
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let running = f.wait(&id, |a| a["state"] == "running");
    fs::write(&source, &bytes).unwrap();
    let redirect=f.ok(json!({"op":"send","target":id,"body":"reply:redirect","mode":"redirect","attachments":[source]}));
    delivery(&f, &id, &redirect, "completed");
    let history = inputs(&f, provider);
    assert_image(history.last().unwrap(), provider, &bytes);
    assert_eq!(history.last().unwrap()["thread"], running["thread_id"]);
    let connection = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let count: u32 = connection
        .query_row(
            "SELECT images FROM agent_image_usage WHERE agent=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 3,
        "accepted replay must not charge another occurrence"
    );
    f.shutdown();
}

#[test]
fn attachments_steer_keeps_references_on_the_same_turn_without_native_blocks() {
    let mut f = Harness::new();
    let path = f.tmp.path().join("guidance.png");
    fs::write(&path, png()).unwrap();
    f.start();
    let id = f.spawn("hold");
    let running = f.wait(&id, |a| a["state"] == "running");
    let request = f
        .ok(json!({"op":"send","target":id,"body":"guidance","mode":"steer","attachments":[path]}));
    let row = delivery(&f, &id, &request, "submitted");
    assert_eq!(
        row["attachments"][0]["delivery"],
        json!({"kind":"reference","reason":"steering"})
    );
    assert_eq!(f.status(&id)["turn_id"], running["turn_id"]);
    let history = inputs(&f, Provider::Codex);
    assert_eq!(history.len(), 1); // Steering never opened a new native user turn.
    let steers = fs::read_to_string(f.provider_home(Provider::Codex).join("steers.jsonl")).unwrap();
    let steer: Value = serde_json::from_str(steers.lines().last().unwrap()).unwrap();
    assert_eq!(steer["input"].as_array().unwrap().len(), 1);
    assert_eq!(steer["input"][0]["type"], "text");
    assert!(
        steer["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Hive attachment references")
    );
    assert!(
        f.read(&id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "steering_submitted")
    );
    f.shutdown();
}

#[test]
fn attachments_purge_reclaims_only_the_last_copy_and_reports_unique_bytes() {
    let mut f = Harness::new();
    let source = f.tmp.path().join("report.txt");
    fs::write(&source, "snapshot").unwrap();
    f.start();
    let mut agents = Vec::new();
    for _ in 0..2 {
        let spawn = f.ok(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"complete","attachments":[source]}));
        let id = spawn["agent_id"].as_str().unwrap().to_owned();
        f.wait(&id, |a| {
            a["state"] == "stopped" && a["task_outcome"] == "completed"
        });
        super::purge::purgeable(&f, &id);
        agents.push(id);
    }
    let status = f.ok(json!({"op":"status","target":agents[1]}));
    assert_eq!(status["attachment_store_bytes"], 8);
    let copy = PathBuf::from(
        status["latest_attachments"]["attachments"][0]["source"]["path"]
            .as_str()
            .unwrap(),
    );
    let first = f.ok(json!({"op":"purge","target":agents[0]}));
    assert_eq!(first["counts"]["attachments"], 1);
    assert_eq!(first["counts"]["attachment_bytes"], 0);
    assert_eq!(fs::read(&copy).unwrap(), b"snapshot");
    let last = f.ok(json!({"op":"purge","target":agents[1]}));
    assert_eq!(last["counts"]["attachments"], 1);
    assert_eq!(last["counts"]["attachment_bytes"], 8);
    assert!(!copy.exists());
    assert_eq!(fs::read(source).unwrap(), b"snapshot");
    f.shutdown();
}
