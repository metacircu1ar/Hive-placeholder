use super::*;
use crate::cli::{Command, response_tests::Stub};
use clap::Parser;

#[test]
fn sandbox_settings_warn_about_full_access_roles_or_spawn_widening() {
    for (roles, expected_roles) in [
        ("normal: {}", vec![]),
        (
            "normal: {}, unrestricted: {sandbox: full-access}",
            vec!["unrestricted"],
        ),
    ] {
        for allow_spawn_widening in [false, true] {
            let config = Config::parse(&format!(
                "auth_profiles: {{main: {{provider: codex, method: subscription}}}}\nproviders: {{codex: {{model: luna}}}}\nroles: {{{roles}}}\nruntime: {{allow_spawn_widening: {allow_spawn_widening}}}\n"
            )).unwrap();
            let mut report = Report::default();
            sandbox_settings(&config, &mut report);
            assert_eq!(report.checks.len(), 1);
            assert_eq!(report.checks[0].name, "sandbox");
            assert!(
                report.checks[0].status
                    == if allow_spawn_widening || !expected_roles.is_empty() {
                        Status::Warning
                    } else {
                        Status::Ok
                    }
            );
            assert_eq!(
                report.checks[0].detail,
                json!({"full_access_roles": expected_roles, "allow_spawn_widening": allow_spawn_widening})
            );
        }
    }
}

#[test]
fn broken_config_does_not_hide_unreachable_daemon() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.yaml");
    std::fs::write(&config, "unknown_setup_key: true").unwrap();
    let cli = Cli::try_parse_from(["hivectl", "doctor"]).unwrap();
    assert!(matches!(cli.command, Command::Doctor));
    let result = run(&cli, &config, &temp.path().join("state"));
    assert!(!result.success);
    let checks = result.value["checks"].as_array().unwrap();
    for name in ["config", "daemon"] {
        assert!(
            checks
                .iter()
                .any(|c| c["name"] == name && c["status"] == "error")
        );
    }
    assert!(!temp.path().join("state").exists());
}

#[test]
fn doctor_rejects_different_daemon_product_or_features() {
    let cli = Cli::try_parse_from(["hivectl", "doctor"]).unwrap();
    for (version, features, ok) in [
        (
            env!("CARGO_PKG_VERSION"),
            runtime::features::COMPILED.to_vec(),
            true,
        ),
        ("999.0", runtime::features::COMPILED.to_vec(), false),
        (env!("CARGO_PKG_VERSION"), vec!["wrong-feature"], false),
    ] {
        let stub = Stub::new(vec![
            json!({"status":"running","product_version":version,"compiled_features":features}),
        ]);
        let mut report = Report::default();
        daemon(
            &cli,
            Path::new("/config path/it's.yaml"),
            stub.root(),
            None,
            &mut report,
        );
        assert_eq!(report.checks[0].status == Status::Ok, ok);
        if !ok {
            assert!(report.checks[0].fixes[0].contains("'\\''"));
        }
        assert_eq!(stub.finish().len(), 1);
    }
}
