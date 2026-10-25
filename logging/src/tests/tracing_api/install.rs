use super::*;
use std::process::Command;
struct ExistingLog;
impl log::Log for ExistingLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, _: &log::Record<'_>) {}
    fn flush(&self) {}
}
static EXISTING: ExistingLog = ExistingLog;
#[test]
fn global_registration_and_conflicts_have_explicit_ownership() {
    if let Some(mode) = std::env::var_os("HIVE_TRACING_INSTALL_CHILD") {
        child(mode.to_str().unwrap());
        return;
    }
    for mode in ["fresh", "log_conflict", "subscriber_conflict"] {
        let output = Command::new(std::env::current_exe().unwrap()).args(["--exact", "tests::tracing_api::install::global_registration_and_conflicts_have_explicit_ownership", "--nocapture"]).env("HIVE_TRACING_INSTALL_CHILD", mode).output().unwrap();
        assert!(
            output.status.success(),
            "mode {mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
fn child(mode: &str) {
    let (backend, guard, memory) = scoped(Config {
        level: LevelFilter::TRACE,
        ..Config::default()
    });
    match mode {
        "fresh" => {
            backend.install_global().unwrap();
            log::debug!("{}", panic_count());
            log::trace!("{}", panic_count());
            log::warn!("sk-secret-dependency-warning");
            tracing::info!(name: "application.ready", count = 1);
            assert_eq!(
                guard.finish(Duration::from_secs(2)).state,
                crate::Shutdown::Drained
            );
            assert_eq!(memory.records().len(), 2);
            assert!(
                !serde_json::to_string(&memory.records())
                    .unwrap()
                    .contains("sk-secret")
            );
        }
        "log_conflict" => {
            log::set_logger(&EXISTING).unwrap();
            assert!(matches!(
                backend.install_global(),
                Err(crate::InstallError::BridgeConflict(_))
            ));
            assert!(
                tracing::subscriber::set_global_default(tracing_subscriber::registry()).is_ok()
            );
            assert_eq!(
                guard.finish(Duration::from_secs(2)).state,
                crate::Shutdown::Drained
            );
            assert!(memory.records().is_empty());
        }
        "subscriber_conflict" => {
            let (existing, old_guard, old_memory) = scoped(Config::default());
            tracing::subscriber::set_global_default(tracing_subscriber::registry().with(existing))
                .unwrap();
            assert!(matches!(
                backend.install_global(),
                Err(crate::InstallError::SubscriberConflictWithBridgeInstalled(
                    _
                ))
            ));
            log::warn!("sk-secret-remains-private");
            assert_eq!(
                guard.finish(Duration::from_secs(2)).state,
                crate::Shutdown::Drained
            );
            assert!(memory.records().is_empty());
            assert_eq!(
                old_guard.finish(Duration::from_secs(2)).state,
                crate::Shutdown::Drained
            );
            assert_eq!(old_memory.records().len(), 1);
        }
        _ => panic!("unknown child mode"),
    }
}
