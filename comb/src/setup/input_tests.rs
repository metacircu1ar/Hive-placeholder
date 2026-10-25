use super::*;
use crate::embedding::Config;
use crate::test_support::embedding_http;
use std::{fs, path::PathBuf};

fn invalid(options: Options, field: &str) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("poisoned-root");
    fs::write(&root, b"untouched").unwrap();
    let error = Plan::inspect(&root, None, &options).err().unwrap();
    assert_eq!(error.input().unwrap().field(), format!("/options/{field}"));
    assert!(!error.input().unwrap().examples().is_empty());
    assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
    assert_eq!(fs::read(&root).unwrap(), b"untouched");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn option_rules_run_before_poisoned_root_inspection() {
    invalid(
        Options {
            services: true,
            no_services: true,
            ..Options::default()
        },
        "services",
    );
    invalid(
        Options {
            service_environment: Some("missing".into()),
            no_services: true,
            ..Options::default()
        },
        "service_environment",
    );
    invalid(
        Options {
            service_path: Some("/bin".into()),
            no_services: true,
            ..Options::default()
        },
        "service_path",
    );
    invalid(
        Options {
            bridge_file: Some("missing".into()),
            ..Options::default()
        },
        "bridge_file",
    );
    invalid(
        Options {
            reviewer_role: vec!["reviewer".into()],
            ..Options::default()
        },
        "reviewer_role",
    );
    invalid(
        Options {
            reviewer_role: vec!["same".into(), "same".into()],
            for_hive: true,
            ..Options::default()
        },
        "reviewer_role",
    );
}

fn config(url: &str) -> serde_json::Value {
    serde_json::json!({"backend":"http","model":"fixture","revision":"original","dimensions":2,"url":url,"timeout_seconds":1})
}

#[test]
fn malformed_config_and_http_scalars_precede_root_and_asset_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config");
    fs::write(&file, b"PRIVATE: [").unwrap();
    invalid(
        Options {
            embedding_config: Some(file.clone()),
            ..Options::default()
        },
        "embedding_config",
    );
    for (field, value) in [
        ("url", serde_json::json!("http://PRIVATE.invalid")),
        ("model", serde_json::json!("")),
        ("dimensions", serde_json::json!(0)),
        ("timeout_seconds", serde_json::json!(121)),
        ("query_prefix", serde_json::json!("PRIVATE".repeat(200))),
    ] {
        let mut raw = config("http://127.0.0.1:9/v1/embeddings");
        raw[field] = value;
        raw["tokenizer"] = serde_json::json!("missing-tokenizer");
        raw["max_tokens"] = serde_json::json!(32);
        fs::write(&file, raw.to_string()).unwrap();
        invalid(
            Options {
                embedding_config: Some(file.clone()),
                ..Options::default()
            },
            "embedding_config",
        );
    }
}

#[test]
fn retained_http_config_survives_replacement_and_deletion_through_apply() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("root");
    let path = dir.path().join("config");
    let server = embedding_http::Server::new();
    fs::write(&path, config(&server.url).to_string()).unwrap();
    let input = Options {
        embedding_config: Some(path.clone()),
        no_services: true,
        ..Options::default()
    }
    .prepare()
    .unwrap();
    fs::write(&path, b"PRIVATE changed invalid content").unwrap();
    let preview = Plan::inspect_prepared(&root, None, &input).unwrap();
    assert_eq!(preview.http.unwrap().revision, "original");
    fs::remove_file(&path).unwrap();
    let applied = input.clone().run(&root, None).unwrap();
    assert!(applied.changed);
    let Some(Config::Http(saved)) = provision::settings(&root).unwrap().unwrap().embeddings else {
        panic!("HTTP selection")
    };
    assert_eq!(saved.revision, "original");
    assert_eq!(saved.url, server.url);
    assert!(!path.exists());
    // Caller input retention must not skip fresh root/ownership checks.
    fs::write(root.join(".comb/serve.json"), b"broken stored state").unwrap();
    let error = input.run(&root, None).err().unwrap();
    assert!(error.input().is_none());
}

#[test]
fn prepared_config_presence_and_backend_are_checked() {
    assert!(serde_json::from_value::<Config>(serde_json::json!({"backend":"local","model":"fixture","directory":"/assets","dimensions":2})).is_err());
    let options = Options {
        embedding_config: Some(PathBuf::from("selected")),
        ..Options::default()
    };
    let invalid: Config = serde_json::from_value(config("http://PRIVATE.invalid")).unwrap();
    let error = Prepared::from_config(options, Some(invalid)).err().unwrap();
    assert_eq!(error.input().unwrap().field(), "/options/embedding_config");
    let error = Prepared::from_config(
        Options::default(),
        Some(serde_json::from_value(config("http://127.0.0.1:9")).unwrap()),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, crate::ErrorCode::InternalError);
}
