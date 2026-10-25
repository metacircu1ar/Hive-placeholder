use super::*;
use std::os::unix::fs::symlink;

#[test]
fn shared_regular_config_and_symlink_remain_supported()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("config");
    let link = dir.path().join("link");
    std::fs::write(&file, b"configuration")?;
    symlink(&file, &link)?;
    regular_config(Some(&file))?;
    regular_config(Some(&link))?;
    regular_config(None)?;
    assert_eq!(
        regular_config(Some(dir.path()))
            .err()
            .ok_or("directory accepted")?
            .code(),
        "invalid_file"
    );
    Ok(())
}

#[test]
fn apply_rechecks_selected_path_before_spawning_child()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("config");
    std::fs::write(&config, b"configuration")?;
    regular_config(Some(&config))?;
    let plan = Comb {
        binary: dir.path().join("missing-binary"),
        args: vec![],
        details: Details {
            root: dir.path().into(),
            bridge_file: None,
        },
        embedding_config: Some(config.clone()),
        inputs: vec!["--embedding-config"],
    };
    std::fs::remove_file(&config)?;
    std::fs::create_dir(&config)?;
    let error = plan.apply().err().ok_or("changed path accepted")?;
    assert_eq!(error.code(), "invalid_file");
    assert_eq!(serde_json::to_value(error)?["field"], "--embedding-config");
    Ok(())
}
