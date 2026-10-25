use super::{catalog, parse};
use std::ffi::OsString;

#[test]
fn globals_are_checked_for_every_public_command_in_both_positions() {
    for (path, _, example) in catalog::COMMANDS {
        if matches!(*path, "" | "model" | "credential" | "service") {
            continue;
        }
        let mut args: Vec<_> = example.split_whitespace().skip(1).collect();
        if *path == "init" {
            args.pop(); // Test global root independently of the positional DIR conflict.
        }
        if matches!(*path, "serve" | "consolidate" | "logs") {
            args.drain(..2); // Explicit globals are varied by the matrix below.
        }
        check_command(path, &args);
    }
    check_command("sync", &["sync"]);
    check_command("setup-maintenance", &["setup-maintenance"]);
}

fn check_command(path: &str, base: &[&str]) {
    for flag in ["--root", "--socket", "--run", "--managed-service"] {
        for before in [true, false] {
            let mut args = vec!["comb"];
            if before {
                args.extend([flag, "PRIVATE"]);
            }
            args.extend_from_slice(base);
            if !before {
                args.extend([flag, "PRIVATE"]);
            }
            if path == "consolidate" && flag != "--run" {
                args.extend(["--run", "curate"]);
            }
            let allowed = applicable(flag, path, base.contains(&"--rebuild"));
            let parsed = parse(&args.iter().map(OsString::from).collect::<Vec<_>>());
            if allowed {
                assert!(parsed.is_ok(), "{args:?}");
            } else {
                let output = parsed.err().expect("unused global must be rejected");
                assert_eq!(output.exit_code, 2, "{args:?}");
                let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
                assert_eq!(value["error"]["field"], flag, "{args:?}");
                assert_eq!(value["error"]["code"], "conflicting_arguments");
                let example = &catalog::examples(path).unwrap().as_slice()[0].clone();
                assert!(output.stderr.contains(example));
                assert!(!output.stdout.contains("PRIVATE"));
                assert!(!output.stderr.contains("PRIVATE"));
            }
        }
    }
}

fn applicable(flag: &str, path: &str, rebuild: bool) -> bool {
    let command = path.split_whitespace().next().unwrap();
    match flag {
        "--root" => !matches!(command, "version" | "mcp"),
        "--socket" => {
            !matches!(
                command,
                "version"
                    | "init"
                    | "upgrade"
                    | "embed"
                    | "model"
                    | "service"
                    | "setup-maintenance"
                    | "logs"
            ) && !rebuild
        }
        "--run" => matches!(
            command,
            "put" | "append" | "capture" | "promote" | "consolidate" | "decide"
        ),
        "--managed-service" => command == "serve",
        _ => panic!("unrecognized test flag"),
    }
}

#[test]
fn model_commands_keep_root_applicable_and_help_keeps_short_circuiting() {
    for args in [
        vec!["comb", "--root", "PRIVATE", "model", "use", "none"],
        vec!["comb", "model", "use", "none", "--root", "PRIVATE"],
    ] {
        assert!(parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).is_ok());
    }
    for args in [
        vec!["comb", "--root", "PRIVATE", "--version"],
        vec!["comb", "version", "--socket", "PRIVATE", "--help"],
    ] {
        let output = parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>())
            .err()
            .unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
    }
}
