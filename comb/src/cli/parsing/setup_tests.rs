use super::{parse, tests::rejected};
use std::ffi::OsString;

#[test]
fn setup_choices_and_conflicts_have_private_field_errors() {
    for (args, field) in [
        (vec!["setup", "--services", "--no-services"], "--services"),
        (vec!["setup", "--bridge-file", "/PRIVATE"], "--for-hive"),
        (
            vec![
                "setup",
                "--reviewer-role",
                "PRIVATE",
                "--for-hive",
                "--reviewer-role",
                "PRIVATE",
            ],
            "--reviewer-role",
        ),
    ] {
        let output = rejected(&args);
        let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(output.exit_code, 2);
        assert_eq!(value["error"]["field"], field);
        assert!(value["error"]["examples"].is_array());
        assert!(!output.stdout.contains("PRIVATE"));
        assert!(!output.stderr.contains("PRIVATE"));
    }
}

#[test]
fn hive_forwarded_setup_shapes_parse_in_preview_and_apply() {
    // The flags assembled by Hive's setup/comb.rs, including internal controls.
    // Iterate independent option groups while omitting deliberately invalid combinations.
    for choice in [vec![], vec!["--embedding-config", "/config.yaml"]] {
        for services in [
            vec!["--no-services"],
            vec![
                "--service-environment",
                "/environment",
                "--service-path",
                "/usr/bin:/bin",
            ],
            vec![],
        ] {
            for roles in [vec![], vec!["--reviewer-role", "comb-reviewer"]] {
                for dry in [false, true] {
                    let mut args = vec![
                        "comb",
                        "--root",
                        "/knowledge",
                        "setup",
                        "--for-hive",
                        "--yes",
                        "--bridge-file",
                        "/bridge",
                        "--preserve-stopped-services",
                    ];
                    args.extend(choice.clone());
                    args.extend(services.clone());
                    args.extend(roles.clone());
                    if dry {
                        args.push("--dry-run");
                    }
                    let parsed = parse(&args.iter().map(OsString::from).collect::<Vec<_>>());
                    assert!(
                        parsed.is_ok(),
                        "{args:?}: {:?}",
                        parsed.err().map(|e| e.stderr)
                    );
                }
            }
        }
    }
}
