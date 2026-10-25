use super::*;
use ::comb::service::auth::{Connection, ConnectionExt as _, Grant};

struct Running {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
fn doctor_checks_global_role_and_collector_bridges_once_per_file() {
    let tmp = tempfile::Builder::new()
        .prefix("doctor-")
        .tempdir_in("/tmp")
        .unwrap();
    let root = tmp.path().join("brain");
    let mut brain = ::comb::Comb::init(&root).unwrap();
    let credential = ::comb::service::offline(
        &mut brain,
        &::comb::service::Operation::CredentialCreate {
            grant: Grant::Bridge {
                name: "doctor".into(),
                reviewer_roles: vec![],
            },
        },
        None,
    )
    .unwrap();
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket: root.join(".comb/service.sock"),
        credential: serde_json::from_value(credential).unwrap(),
    };
    let path = tmp.path().join("bridge.json");
    connection.write(&path).unwrap();
    drop(brain);
    let server = ::comb::service::Server::open(&root, ::comb::service::Options::default()).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let _running = Running {
        stop: Some(stop),
        thread: Some(std::thread::spawn(move || {
            crate::calls::executor()
                .unwrap()
                .block_on(server.run(async {
                    let _ = stopped.await;
                }))
                .unwrap();
        })),
    };
    let missing = tmp.path().join("missing.json");
    let config = Config::parse(&format!("auth_profiles: {{main: {{provider: codex, method: subscription}}}}\ndefault_provider: codex\nproviders: {{codex: {{auth: main, model: luna, effort: low}}}}\ncomb: {{credential_file: {}}}\nroles:\n  reader:\n    comb: {{credential_file: {}}}\ncomb_collectors:\n  notes:\n    credential_file: {}\n    sources: [correction]\n", path.display(), path.display(), missing.display())).unwrap();
    let mut report = Report::default();
    comb::check(&config, &mut report);
    assert_eq!(report.checks.len(), 2);
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "comb:default,role:reader"
                && c.status == Status::Ok
                && c.detail["comb_id"] == connection.credential.comb_id.to_string())
    );
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.name == "comb:collector:notes" && c.status == Status::Error)
    );
    let config = Config::parse(&format!(
        "auth_profiles: {{main: {{provider: codex, method: subscription}}}}\ndefault_provider: codex\nproviders: {{codex: {{auth: main, model: luna, effort: low}}}}\nroles:\n  reader:\n    comb: {{credential_file: {}}}\n",
        missing.display()
    ))
    .unwrap();
    let mut report = Report::default();
    comb::check(&config, &mut report);
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.checks[0].name, "comb:role:reader");
    assert!(report.checks[0].status == Status::Error);
}
