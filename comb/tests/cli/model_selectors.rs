//! CLI rejection precedes reading settings or contacting an endpoint.
use std::process::Command;
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn invalid_model_selectors_do_not_read_or_change_settings() -> Result {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("root");
    let admin = root.join(".comb");
    std::fs::create_dir_all(&admin)?;
    let saved = admin.join("serve.json");
    std::fs::write(&saved, "PRIVATE corrupt settings")?;
    for name in ["", "default", "PRIVATE-model"] {
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .env_remove("HOME")
            .arg("--root")
            .arg(&root)
            .args(["model", "use", name])
            .output()?;
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(value["error"]["field"], "NAME");
        assert!(
            value["error"]["expected"]
                .as_str()
                .is_some_and(
                    |expected| expected.contains("use-http") && !expected.contains("catalog")
                )
        );
        assert!(
            !value["error"]["examples"]
                .as_array()
                .ok_or("missing examples")?
                .is_empty()
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
    }
    assert_eq!(std::fs::read_to_string(saved)?, "PRIVATE corrupt settings");
    assert_eq!(std::fs::read_dir(&admin)?.count(), 1);
    Ok(())
}
