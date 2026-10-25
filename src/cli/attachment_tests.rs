use super::*;
use serde_json::json;

#[test]
fn spawn_inline_body_keeps_literal_bytes_and_rejects_missing_conflicting_or_large_input() {
    let parse = |args: &[&str]| {
        Cli::try_parse_from(
            ["hivectl", "spawn", "--cwd", "/tmp"]
                .into_iter()
                .chain(args.iter().copied()),
        )
    };
    assert!(parse(&[]).is_err());
    assert!(parse(&["--body", "text", "--body-file", "/missing"]).is_err());
    let literal = "a\n'quoted' $(unchanged) 🚀";
    let Command::Spawn(args) = parse(&["--body", literal]).unwrap().command else {
        panic!("spawn")
    };
    assert_eq!(
        serde_json::to_value(args.request().unwrap()).unwrap()["body"],
        literal
    );
    for bad in [String::new(), "a".repeat(BODY_BYTES + 1)] {
        let Command::Spawn(args) = parse(&["--body", &bad]).unwrap().command else {
            panic!("spawn")
        };
        assert!(args.request().is_err());
    }
}

#[test]
fn spawn_and_send_keep_ordered_paths_without_opening_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let body = tmp.path().join("body");
    std::fs::write(&body, "inspect").unwrap();
    let names = ["nonexistent-dir/first.png", "nonexistent-dir/second.txt"];
    let expected: Vec<_> = names
        .iter()
        .map(|p| std::path::absolute(p).unwrap())
        .collect();
    let mut args = vec![
        "hivectl",
        "spawn",
        "--cwd",
        "/tmp",
        "--body-file",
        body.to_str().unwrap(),
    ];
    for name in names {
        args.extend(["--attach", name]);
    }
    let Command::Spawn(spawn) = Cli::try_parse_from(args).unwrap().command else {
        panic!("spawn");
    };
    let value = serde_json::to_value(spawn.request().unwrap()).unwrap();
    assert_eq!(value["attachments"], json!(expected));

    let stub = response_tests::Stub::new(vec![
        json!({"message_id":"m","recipient":"a","delivery_state":"accepted"}),
    ]);
    let send = Cli::try_parse_from([
        "hivectl",
        "send",
        "a",
        "--body-file",
        body.to_str().unwrap(),
        "--attach",
        names[0],
        "--attach",
        names[1],
    ])
    .unwrap();
    let Command::Send(send) = send.command else {
        panic!("send");
    };
    let Request::Send(args) = send.request().unwrap() else {
        panic!("send request");
    };
    send.run(stub.root(), "same-request", args).unwrap();
    assert_eq!(stub.finish()[0]["args"]["attachments"], json!(expected));
}

#[test]
fn batch_paths_use_invoking_directory_not_operations_file_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("operations.json");
    let operations = json!([
        {"op":"spawn","request_id":"spawn","attachments":["missing-a"]},
        {"op":"send","request_id":"send","attachments":["missing-b","/absolute/missing"]}
    ]);
    std::fs::write(&file, operations.to_string()).unwrap();
    let value = batch_operations(&file).unwrap();
    assert_eq!(
        value[0]["attachments"],
        json!([std::path::absolute("missing-a").unwrap()])
    );
    assert_eq!(
        value[1]["attachments"],
        json!([
            std::path::absolute("missing-b").unwrap(),
            "/absolute/missing"
        ])
    );
    assert!(input::attachment_paths("send", &vec![PathBuf::from("missing"); 11]).is_err());
}
