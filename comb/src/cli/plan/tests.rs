use super::*;
use crate::cli::{
    Cli,
    command::{Command, LocalCommand},
};
use clap::Parser;

fn planned(cli: Cli) -> crate::cli::input::Result<Plan> {
    let context = Context {
        comb: cli.comb.unwrap_or_else(|| PathBuf::from(".")),
        socket: cli.socket,
        run: cli.run,
    };
    match Command::try_from(cli.command)? {
        Command::Remote(command) => command.plan(&context),
        Command::Local(_) => panic!("expected a remote command"),
    }
}

#[test]
fn identifiers_and_relations_are_rejected_before_mutation_input() {
    let hash = "a".repeat(64);
    for args in [
        vec!["put", "../bad", "--file", "absent.md"],
        vec!["put", "concepts/one", "--expected-hash", "bad"],
        vec![
            "append",
            "../bad",
            "Text",
            "--source",
            "test",
            "--summary-file",
            "absent.md",
            "--expected-hash",
            &hash,
        ],
        vec![
            "append",
            "concepts/one",
            "Text",
            "--source",
            "test",
            "--summary-file",
            "absent.md",
        ],
        vec!["promote", "concepts/one", "--expected-hash", "bad"],
        vec![
            "decide",
            "concepts/one",
            "accept",
            "--reason",
            "test",
            "--expected-hash",
            "bad",
        ],
        vec!["rm", "concepts/one", "--expected-hash", "bad"],
        vec!["import", "absent", "--prefix", "../bad"],
        vec!["search", "text", "--after", "not-a-date"],
        vec!["--run", "\n", "capture", "Text"],
    ] {
        assert!(
            Cli::try_parse_from(["comb"].into_iter().chain(args.clone())).is_err(),
            "{args:?}"
        );
    }
}

#[test]
fn valid_import_plan_canonicalizes_the_directory_and_preserves_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::try_parse_from([
        "comb",
        "import",
        dir.path().to_str().unwrap(),
        "--prefix",
        "concepts",
    ])
    .unwrap();
    let Operation::Import { directory, prefix } = planned(cli).unwrap().operation else {
        panic!("import");
    };
    assert_eq!(directory, dir.path().canonicalize().unwrap());
    assert_eq!(prefix.unwrap().as_str(), "concepts");
}

#[test]
fn generated_request_id_is_stable_for_the_completed_plan_and_attributed_afterward() {
    let dir = tempfile::tempdir().unwrap();
    let context = Context {
        comb: dir.path().join("absent"),
        socket: None,
        run: None,
    };
    let cli = Cli::try_parse_from(["comb", "capture", "Text"]).unwrap();
    let plan = planned(cli).unwrap();
    let id = plan.operation.request_id().unwrap().to_owned();
    assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 4);
    let error = plan.execute(&context).unwrap_err();
    assert_eq!(error.request_id.as_deref(), Some(id.as_str()));
    assert!(!error.file_committed);
    assert!(
        Cli::try_parse_from([
            "comb",
            "capture",
            "Text",
            "--request-id",
            "explicit\nidentity",
        ])
        .is_err()
    );
}

#[test]
fn only_rebuild_sync_is_classified_as_local() {
    let ordinary = Cli::try_parse_from(["comb", "sync"]).unwrap();
    assert!(matches!(
        Command::try_from(ordinary.command).unwrap(),
        Command::Remote(RemoteCommand::Sync)
    ));
    let rebuild = Cli::try_parse_from(["comb", "sync", "--rebuild"]).unwrap();
    assert!(matches!(
        Command::try_from(rebuild.command).unwrap(),
        Command::Local(LocalCommand::Rebuild)
    ));
}

#[test]
fn prepared_query_config_is_not_reopened_before_execution() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("absent");
    let path = dir.path().join("embedding.json");
    let missing_key = format!("COMB_ONCE_{}", uuid::Uuid::new_v4().simple());
    std::fs::write(
        &path,
        serde_json::json!({"backend":"http","model":"test","revision":"pin","dimensions":3,
            "url":"http://127.0.0.1:9/v1/embeddings","api_key_env":missing_key})
        .to_string(),
    )
    .unwrap();
    let cli = Cli::try_parse_from([
        std::ffi::OsStr::new("comb"),
        std::ffi::OsStr::new("--root"),
        root.as_os_str(),
        std::ffi::OsStr::new("search"),
        std::ffi::OsStr::new("question"),
        std::ffi::OsStr::new("--embedding-config"),
        path.as_os_str(),
    ])
    .unwrap();
    let plan = planned(cli).unwrap();
    std::fs::remove_file(&path).unwrap();
    let error = plan
        .execute(&Context {
            comb: root.clone(),
            socket: None,
            run: None,
        })
        .unwrap_err();
    assert_eq!(error.code, comb::ErrorCode::EmbeddingCredentials);
    assert!(!root.exists());
}

#[test]
fn import_admin_modes_require_exactly_one_action_before_store_access() {
    let id = "12345678-1234-4234-8234-123456789abc";
    for args in [
        vec!["import"],
        vec!["import", "absent", "--status"],
        vec!["import", "absent", "--abandon", id],
        vec!["import", "--status", "--abandon", id],
        vec!["import", "--status", "--prefix", "notes"],
        vec!["import", "--abandon", id, "--prefix", "notes"],
        vec!["import", "--abandon", "not-an-id"],
    ] {
        assert!(Cli::try_parse_from(["comb"].into_iter().chain(args)).is_err());
    }
    let status = Cli::try_parse_from(["comb", "--root", "/absent", "import", "--status"]).unwrap();
    assert!(matches!(
        planned(status).unwrap().operation,
        Operation::ImportStatus
    ));
    let abandon = Cli::try_parse_from(["comb", "import", "--abandon", id]).unwrap();
    assert!(
        matches!(planned(abandon).unwrap().operation, Operation::ImportAbandon { batch_id } if batch_id.as_str() == id)
    );
}
