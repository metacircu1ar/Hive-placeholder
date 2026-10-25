use super::*;

#[test]
fn separate_cli_processes_share_cas_receipts_and_rebuildable_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    cli(root, &["init"], None, true).unwrap();
    let page = "---\ntitle: Writer\ntype: component\n---\nWriter commits transactions.\n";
    let created = create_and_append(root, page).unwrap();
    let conflict = cli(
        root,
        &[
            "put",
            "components/writer",
            "--expected-hash",
            created["hash"].as_str().unwrap(),
        ],
        Some(page),
        false,
    )
    .unwrap();
    assert_eq!(conflict["error"]["code"], "conflict");
    assert!(uuid::Uuid::parse_str(conflict["error"]["request_id"].as_str().unwrap()).is_ok());
    cli(root, &["sync", "--rebuild"], None, true).unwrap();
    let doc = cli(root, &["get", "components/writer"], None, true).unwrap();
    assert_eq!(doc["page"]["timeline"].as_array().unwrap().len(), 1);
    assert_eq!(doc["unverified"], false);
    let results = cli(root, &["search", "latency"], None, true).unwrap();
    assert_eq!(results["results"][0]["slug"], "components/writer");
    let health = cli(root, &["doctor"], None, true).unwrap();
    assert!(
        !health["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "dangling_citation")
    );
    cli(
        root,
        &[
            "rm",
            "components/writer",
            "--expected-hash",
            doc["hash"].as_str().unwrap(),
        ],
        None,
        true,
    )
    .unwrap();
    assert_eq!(cli(root, &["stats"], None, true).unwrap()["pages"], 0);
}

fn create_and_append(root: &Path, page: &str) -> TestResult<Value> {
    let created = cli(
        root,
        &["put", "components/writer", "--request-id", "create"],
        Some(page),
        true,
    )?;
    let replay = cli(
        root,
        &["put", "components/writer", "--request-id", "create"],
        Some(page),
        true,
    )?;
    assert_eq!(replay["replayed"], true);
    let summary = root.join("summary.txt");
    std::fs::write(&summary, "Sync explains latency. [evidence:new]\n")?;
    cli(
        root,
        &[
            "append",
            "components/writer",
            "Measured disk sync",
            "--source",
            "test:cli",
            "--summary-file",
            summary.to_str().ok_or("summary path is not UTF-8")?,
            "--expected-hash",
            created["hash"].as_str().ok_or("created hash is missing")?,
            "--request-id",
            "append",
        ],
        None,
        true,
    )?;
    Ok(created)
}

#[test]
fn http_embedding_cli_reuses_cache_after_rebuild_and_reports_pending_input() {
    let server = embedding_http::Server::new();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("comb");
    let config = http_config(dir.path(), &server.url).unwrap();
    let config = config.to_str().unwrap();
    cli(&root, &["init"], None, true).unwrap();
    cli(
        &root,
        &["put", "concepts/cat"],
        Some("---\ntitle: Cat\ntype: concept\n---\ncat dog\n"),
        true,
    )
    .unwrap();
    let embedded = cli(&root, &["embed", "--config", config], None, true).unwrap();
    assert_eq!(embedded["embedded"], 1);
    let results = vector_search(&root, config).unwrap();
    assert_eq!(results["mode"], "vector");
    assert_eq!(results["results"][0]["slug"], "concepts/cat");
    cli(&root, &["sync", "--rebuild"], None, true).unwrap();
    cli(&root, &["embed", "--config", config], None, true).unwrap();
    let again = vector_search(&root, config).unwrap();
    assert_eq!(results, again);
    cli(
        &root,
        &["put", "concepts/dog"],
        Some("---\ntitle: Dog\ntype: concept\n---\ndog cat\n"),
        true,
    )
    .unwrap();
    let partial = cli(
        &root,
        &["search", "cat", "--embedding-config", config],
        None,
        true,
    )
    .unwrap();
    assert_eq!(partial["mode"], "hybrid");
    assert_eq!(partial["coverage"]["pending"], 1);
    assert_eq!(partial["results"].as_array().unwrap().len(), 2);
}

fn http_config(directory: &Path, url: &str) -> TestResult<std::path::PathBuf> {
    let config = directory.join("embeddings.yaml");
    std::fs::write(
        &config,
        serde_json::to_string(&serde_json::json!({
            "backend":"http", "model":"fixture", "revision":"pinned",
            "url":url, "dimensions":2
        }))?,
    )?;
    Ok(config)
}

fn vector_search(root: &Path, config: &str) -> TestResult<Value> {
    cli(
        root,
        &[
            "search",
            "cat",
            "--mode",
            "vector",
            "--embedding-config",
            config,
        ],
        None,
        true,
    )
}
