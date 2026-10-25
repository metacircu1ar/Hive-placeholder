//! Installer-only native purge: preserve logins and exclude the runtime writer.
use super::{CommandOutcome, Config, Error, Result, State, api};
use hive_setup_support::{
    locks,
    maintenance::{Args, Plan, Scope},
    removal::{self, Action, Object},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Details {
    config_digest: Option<String>,
    runtime_admitted: bool,
}
const RUNTIME: &[&str] = &[
    "runtime.sqlite",
    "runtime.sqlite-wal",
    "runtime.sqlite-shm",
    "runtime.sqlite-journal",
    "output",
    "attachments",
    "output-manifest.jsonl",
    "control.sock",
];

pub(super) fn run(config: &Path, state: &Path, args: &Args) -> Result<CommandOutcome> {
    let plan = if let Some(receipt) = &args.apply {
        apply(receipt)?
    } else {
        prepare(config, state, args)?
    };
    Ok(CommandOutcome::ok(api::value(plan)))
}
fn io(error: std::io::Error) -> Error {
    Error::state_io_from(error)
}
fn invalid(message: impl Into<String>) -> Error {
    Error::ConfigInvalid {
        message: message.into().into(),
        source: None,
    }
}
fn prepare(config: &Path, state: &Path, args: &Args) -> Result<Plan<Details>> {
    let config = removal::safe_path(config).map_err(io)?;
    let state = removal::safe_path(state).map_err(io)?;
    removal::disjoint(&[&config, &state]).map_err(io)?;
    let _admission = crate::state::State::new(state.clone())?.admission()?;
    let bytes = match std::fs::read(&config) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io(error)),
    };
    let parsed = if config.try_exists().map_err(io)? {
        Some(Config::load(&config)?)
    } else {
        None
    };
    State::new(state.clone())?.check_format()?;
    crate::runtime::check_existing_format(&state.join("runtime/runtime.sqlite"))?;
    let (bridge, root) = binding(args, parsed.as_ref())?;
    let removals = vec![Action::capture(&config, "config").map_err(io)?];
    let retained = [&state, &state.join("runtime")]
        .into_iter()
        .map(|p| Object::capture(p))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(io)?;
    Ok(Plan {
        format: hive_input::compatibility::Exact,
        product: "Hive".into(),
        scope: Scope {
            config: Some(config),
            state: Some(state),
            root,
            bridge,
        },
        retained,
        removals,
        details: Details {
            config_digest: bytes.map(|b| format!("{:x}", Sha256::digest(b))),
            runtime_admitted: false,
        },
    })
}
fn binding(args: &Args, config: Option<&Config>) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    #[cfg(feature = "comb")]
    let configured = config.and_then(|c| c.comb.as_ref().map(|b| b.credential_file.clone()));
    #[cfg(not(feature = "comb"))]
    let configured: Option<PathBuf> = {
        let _ = config;
        None
    };
    let bridge = args
        .bridge_file
        .clone()
        .or(configured)
        .map(|p| removal::safe_path(&p).map_err(io))
        .transpose()?;
    let root = args.knowledge_root.clone();
    #[cfg(feature = "comb")]
    let mut root = root;
    #[cfg(feature = "comb")]
    if root.is_none()
        && let Some(path) = bridge.as_ref().filter(|p| p.exists())
    {
        let connection = comb_protocol::service::auth::Connection::read(path)
            .map_err(|cause| invalid("Cannot read Comb connection.").with_source(cause))?;
        root = connection
            .socket
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == ".comb"))
            .and_then(Path::parent)
            .map(Path::to_owned);
        if root.is_none() {
            return Err(invalid(
                "Custom Comb socket: pass --comb-root before purging the Hive bridge.",
            ));
        }
    }
    Ok((bridge, root))
}
fn apply(receipt: &Path) -> Result<Plan<Details>> {
    let mut plan = Plan::<Details>::read(receipt, "Hive").map_err(io)?;
    let config = plan
        .scope
        .config
        .clone()
        .ok_or_else(|| invalid("Missing purge configuration scope."))?;
    let state = plan
        .scope
        .state
        .clone()
        .ok_or_else(|| invalid("Missing purge state scope."))?;
    removal::disjoint(&[&config, &state]).map_err(io)?;
    if plan
        .retained
        .iter()
        .map(|o| &o.path)
        .ne([&state, &state.join("runtime")])
    {
        return Err(invalid("Invalid retained runtime scope."));
    }

    let _config_lock = config
        .parent()
        .filter(|p| p.exists())
        .map(|_| locks::path(&config, "hive-setup", false))
        .transpose()
        .map_err(io)?;
    plan.check_retained().map_err(io)?;
    let _daemon = if state.join("runtime").exists() {
        Some(if plan.details.runtime_admitted {
            crate::runtime::purge_lock(&state)?
        } else {
            crate::runtime::maintenance_lock(&state)?
        })
    } else {
        None
    };
    plan.check_retained().map_err(io)?;
    if !plan.details.runtime_admitted {
        admit_runtime(&mut plan, &state)?;
        plan.details.runtime_admitted = true;
        plan.save(receipt).map_err(io)?;
    }
    let expected: Vec<_> = std::iter::once(config.clone())
        .chain(RUNTIME.iter().map(|n| state.join("runtime").join(n)))
        .collect();
    if plan
        .removals
        .iter()
        .map(|a| &a.object.path)
        .ne(expected.iter())
    {
        return Err(invalid(
            "Purge receipt does not match native runtime ownership.",
        ));
    }
    if !plan.removals[0].done && config.exists() {
        let bytes = std::fs::read(&config).map_err(io)?;
        if Some(format!("{:x}", Sha256::digest(bytes))) != plan.details.config_digest {
            return Err(invalid("Configuration changed after purge admission."));
        }
    }
    // Schema was checked at admission. A partial purge may already have removed
    // SQLite/WAL files; recovery must finish the exact recorded objects only.
    for index in 0..plan.removals.len() {
        if plan.removals[index].done {
            plan.removals[index].remove().map_err(io)?;
        } else {
            plan.remove(index, receipt).map_err(io)?;
        }
    }
    Ok(plan)
}

fn admit_runtime(plan: &mut Plan<Details>, state: &Path) -> Result<()> {
    for name in RUNTIME {
        let path = state.join("runtime").join(name);
        let mut action = Action::capture(&path, "runtime").map_err(io)?;
        if plan.retained[1].identity.is_none() {
            action.object.identity = None;
        } else {
            removal::tree(&path).map_err(io)?;
        }
        plan.removals.push(action);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::MetadataExt};
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.yaml");
        fs::write(&config, "auth_profiles: {}\n").unwrap();
        let state = temp.path().join("state");
        drop(State::new(state.clone()).unwrap().initialize().unwrap());
        crate::state::directory(&state.join("runtime")).unwrap();
        (temp, config, state)
    }
    #[test]
    fn purge_preserves_login_paths_and_lock_inode_and_resumes_after_config_removal() {
        let (temp, config, state) = fixture();
        let database = state.join("runtime/runtime.sqlite");
        drop(crate::state::private_file(&database, true).unwrap());
        let connection = rusqlite::Connection::open(database).unwrap();
        crate::runtime::initialize_schema(&connection).unwrap();
        drop(connection);
        crate::state::directory(&state.join("profiles")).unwrap();
        fs::write(state.join("profiles/native-history"), b"keep").unwrap();
        let mut plan = prepare(&config, &state, &Args::default()).unwrap();
        let receipt = temp.path().join("purge.json");
        plan.save(&receipt).unwrap();
        let lock = crate::runtime::maintenance_lock(&state).unwrap();
        let inode = fs::metadata(state.join("runtime/daemon.lock"))
            .unwrap()
            .ino();
        assert!(apply(&receipt).is_err());
        drop(lock);
        plan.remove(0, &receipt).unwrap(); // Crash after config deletion, before runtime admission.
        apply(&receipt).unwrap();
        assert!(!state.join("runtime/runtime.sqlite").exists());
        assert_eq!(
            fs::read(state.join("profiles/native-history")).unwrap(),
            b"keep"
        );
        assert_eq!(
            fs::metadata(state.join("runtime/daemon.lock"))
                .unwrap()
                .ino(),
            inode
        );
        apply(&receipt).unwrap();
        fs::write(&config, b"new configuration").unwrap();
        assert!(apply(&receipt).is_err());
        assert_eq!(fs::read(config).unwrap(), b"new configuration");
    }
    #[test]
    fn initially_absent_runtime_never_admits_concurrently_created_files() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let config = temp.path().join("config.yaml");
        let mut plan = prepare(&config, &state, &Args::default()).unwrap();
        fs::create_dir_all(state.join("runtime")).unwrap();
        let database = state.join("runtime/runtime.sqlite");
        fs::write(&database, b"new daemon").unwrap();
        admit_runtime(&mut plan, &state).unwrap();
        assert!(plan.removals[1].object.identity.is_none());
        assert!(plan.removals[1].remove().is_err());
        assert_eq!(fs::read(database).unwrap(), b"new daemon");
    }
}
