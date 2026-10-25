use super::*;
use crate::auth::{Auth, Report, Settings};
use crate::config::{Config, Method, Profile, Provider};
use crate::state::sync_tests::with_hook;
use atomic_fs::Step;
use std::time::Duration;
use std::{io, path::Path};
#[test]
fn repair_report_preserves_probe_failure_and_accurate_retained_path() {
    if std::env::var_os("HIVE_PROBE_REPORT_FIXTURE").is_none() {
        crate::test_support::run_isolated(crate::test_support::rerun_isolated(
            "auth::tests::probe_cleanup::repair_report_preserves_probe_failure_and_accurate_retained_path",
            false,
        ).env("HIVE_PROBE_REPORT_FIXTURE", "1"));
        return;
    }
    // Keep the real repair_version orchestration; only use an owned /usr/bin/false
    // process, never provider accounts or downloaded model code.
    for removed in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let profile = Profile {
            provider: Provider::Claude,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        };
        let auth = Auth {
            config: Config::parse("auth_profiles: {}").unwrap(),
            state: State::new(tmp.path().join("old-state")).unwrap(),
            settings: Settings {
                launcher: crate::launch::test_cache().unwrap(),
                codex: "unused".into(),
                claude: "unused".into(),
                probe_timeout: Duration::from_secs(5),
                login_timeout: Duration::from_secs(5),
                browser: false,
                reauthenticate: true,
                cancellation: crate::process::Cancellation::install().unwrap(),
            },
        };
        let mut report = Report::new("main", &profile, None);
        let point = if removed {
            Step::BeforeDirectorySync
        } else {
            Step::BeforeRemove
        };
        let result = with_hook(
            move |path, step| {
                if path
                    .file_name()
                    .is_some_and(|name| name.to_str().unwrap().starts_with(".hive-probe-"))
                    && step == point
                {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || {
                auth.repair_version(
                    Path::new("/usr/bin/false"),
                    Some(&tmp.path().join("replacement")),
                    &mut report,
                )
            },
        );
        assert_eq!(result.unwrap_err().code(), "provider_failed");
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].io_raw_os_error(), Some(libc::EIO));
        assert_eq!(report.metadata_probe_directory.is_none(), removed);
        if let Some(path) = report.metadata_probe_directory {
            assert!(path.exists());
        }
        assert!(!tmp.path().join("replacement").exists());
    }
}
