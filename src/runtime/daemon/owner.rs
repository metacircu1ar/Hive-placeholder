//! Durable daemon ownership and maintenance exclusion checks.
use crate::{
    error::{Error, Result},
    runtime::invalid_request,
    state::{self, Lock, State},
};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;
use std::{io::Read, path::Path};
use uuid::Uuid;

// Maximum daemon ownership metadata read before deciding whether recovery is needed.
const DAEMON_MARKER_READ_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DaemonOwner {
    product_version: String,
    binary_sha256: String,
    boot: String,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    identity: state::ProcessIdentity,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    lock: state::LockIdentity,
    pid: u32,
}
impl DaemonOwner {
    fn validate(&self) -> Result<()> {
        Uuid::parse_str(&self.boot).map_err(|cause| invalid_request().with_source(cause))?;
        if self.product_version.is_empty()
            || self.product_version.len() > 64
            || self.product_version.chars().any(char::is_control)
            || self.binary_sha256.len() != 64
            || !self
                .binary_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(state::recovery_required());
        }
        if self.pid == 0 || self.pid > i32::MAX as u32 || self.pid != self.identity.pid {
            return Err(state::recovery_required());
        }
        Ok(())
    }
}
pub(crate) fn check_daemon_owner(marker: &Path, daemon_lock: &Lock) -> Result<()> {
    let present = match marker.symlink_metadata() {
        Ok(_) => true,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => false,
        Err(cause) => return Err(Error::state_io_from(cause)),
    };
    if present {
        let mut reader = serde_json::Deserializer::from_reader(
            state::private_file(marker, false)?.take(DAEMON_MARKER_READ_BYTES),
        );
        let old: DaemonOwner = crate::runtime::domain::decode::object(&mut reader)
            .map_err(|cause| invalid_request().with_source(cause))?;
        reader
            .end()
            .map_err(|cause| invalid_request().with_source(cause))?;
        old.validate()?;
        if state::same_boot(&old.boot)?
            && (!old.identity.absent()?
                || old.lock != state::LockIdentity::from_fd(daemon_lock.fd())?)
        {
            return Err(state::recovery_required());
        }
    }
    Ok(())
}
pub(super) fn write_daemon_owner(
    marker: &Path,
    boot: &str,
    daemon_lock: &Lock,
    binary_sha256: &str,
) -> Result<()> {
    let pid = std::process::id();
    let identity = state::ProcessIdentity::current()?;
    let lock = state::LockIdentity::from_fd(daemon_lock.fd())?;
    state::write_json(
        marker,
        &crate::runtime::api::value(DaemonOwner {
            product_version: env!("CARGO_PKG_VERSION").into(),
            binary_sha256: binary_sha256.into(),
            boot: boot.into(),
            identity,
            lock,
            pid,
        }),
    )
}
/// Exclude controller writes while inspecting generation references for pruning.
pub(crate) fn maintenance_lock(root: &Path) -> Result<Lock> {
    let state = State::new(root.to_owned())?;
    let _initialization = state.initialize()?;
    purge_lock(root)
}
/// Resume an admitted purge whose SQLite files may already be quarantined.
pub(crate) fn purge_lock(root: &Path) -> Result<Lock> {
    let admission = State::new(root.to_owned())?.admission()?;
    state::directory(&root.join("runtime"))?;
    let lock = Lock::acquire(&root.join("runtime/daemon.lock")).map_err(|e| {
        Error::daemon_busy("Stop the daemon before running this maintenance command.")
            .with_source(e)
    })?;
    check_daemon_owner(&root.join("runtime/owner.json"), &lock)?;
    Ok(lock.with_admission(admission))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod owner_tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    const ROOT: &str = "HIVE_OWNER_MARKER_TEST_ROOT";

    #[test]
    fn maintenance_creates_current_marker_before_its_runtime_lock() {
        let temp = crate::runtime::test_directory();
        for existing in [false, true] {
            let root = temp.path().join(if existing { "empty" } else { "absent" });
            if existing {
                state::directory(&root).unwrap();
            }
            let lock = maintenance_lock(&root).unwrap();
            assert!(root.join("format").is_file());
            assert!(root.join("runtime/daemon.lock").is_file());
            State::new(root.clone()).unwrap().check_format().unwrap();
            drop(lock);
            drop(maintenance_lock(&root).unwrap());
        }
    }

    #[test]
    fn unreadable_owner_location_is_not_treated_as_absent() {
        let temp = crate::runtime::test_directory();
        let lock = Lock::acquire(&temp.path().join("daemon.lock")).unwrap();
        check_daemon_owner(&temp.path().join("absent.json"), &lock).unwrap();
        let looped = temp.path().join("loop");
        std::os::unix::fs::symlink("loop", &looped).unwrap();
        let error = check_daemon_owner(&looped.join("owner.json"), &lock).unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::ELOOP));
        assert_eq!(std::fs::read_link(looped).unwrap(), Path::new("loop"));
    }

    #[test]
    #[ignore = "subprocess fixture"]
    fn write_owner_marker_fixture() {
        let root = PathBuf::from(std::env::var_os(ROOT).expect("owner marker fixture root"));
        let lock = Lock::acquire(&root.join("daemon.lock")).unwrap();
        write_daemon_owner(
            &root.join("owner.json"),
            &state::boot_identity().unwrap(),
            &lock,
            &"0".repeat(64),
        )
        .unwrap();
    }

    #[test]
    fn written_owner_marker_reopens_after_exit_on_current_and_other_boots() {
        let temp = crate::runtime::test_directory();
        let (_, module) = module_path!().split_once("::").unwrap();
        let test_name = format!("{module}::write_owner_marker_fixture");
        let mut command = crate::test_support::rerun_isolated(&test_name, true);
        command.arg("--test-threads=1").env(ROOT, temp.path());
        crate::test_support::run_isolated(&mut command);
        let marker = temp.path().join("owner.json");
        let lock = Lock::acquire(&temp.path().join("daemon.lock")).unwrap();
        let mut owner: DaemonOwner =
            serde_json::from_reader(state::private_file(&marker, false).unwrap()).unwrap();
        assert_eq!(owner.boot, state::boot_identity().unwrap());
        assert!(owner.identity.absent().unwrap());
        check_daemon_owner(&marker, &lock).unwrap();
        let boot = Uuid::parse_str(&owner.boot).unwrap();
        owner.boot = Uuid::from_u128(boot.as_u128() ^ 1).to_string();
        state::write_json(&marker, &crate::runtime::api::value(owner)).unwrap();
        check_daemon_owner(&marker, &lock).unwrap();
        write_daemon_owner(
            &marker,
            &state::boot_identity().unwrap(),
            &lock,
            &"0".repeat(64),
        )
        .unwrap();
        assert_eq!(
            check_daemon_owner(&marker, &lock).unwrap_err().code(),
            "recovery_required"
        );
    }

    #[test]
    fn ownership_marker_requires_complete_named_records_on_every_boot() {
        let temp = crate::runtime::test_directory();
        let lock = Lock::acquire(&temp.path().join("daemon.lock")).unwrap();
        let identity = state::ProcessIdentity::current().unwrap();
        let good = crate::runtime::api::value(DaemonOwner {
            product_version: env!("CARGO_PKG_VERSION").into(),
            binary_sha256: "0".repeat(64),
            boot: Uuid::new_v4().to_string(),
            pid: identity.pid,
            identity,
            lock: state::LockIdentity::from_fd(lock.fd()).unwrap(),
        });
        let parse = |value: &Value| crate::runtime::domain::decode::object::<_, DaemonOwner>(value);
        parse(&good).unwrap().validate().unwrap();
        for path in ["", "/identity", "/lock"] {
            let record = good.pointer(path).unwrap();
            for key in record.as_object().unwrap().keys() {
                let mut missing = good.clone();
                missing
                    .pointer_mut(path)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
                assert!(parse(&missing).is_err(), "{path}/{key}");
            }
            let mut unknown = good.clone();
            unknown.pointer_mut(path).unwrap()["extra"] = json!(true);
            assert!(parse(&unknown).is_err());
        }
        for (field, positional) in [
            ("identity", json!([good["pid"], good["identity"]["birth"]])),
            ("lock", json!([good["lock"]["dev"], good["lock"]["ino"]])),
        ] {
            let mut value = good.clone();
            value[field] = positional;
            assert!(parse(&value).is_err());
        }
        for pid in [0, i32::MAX as u32 + 1] {
            let mut value = good.clone();
            value["pid"] = json!(pid);
            assert_eq!(
                parse(&value).unwrap().validate().unwrap_err().code(),
                "recovery_required"
            );
        }
        let mut wrong = good.clone();
        wrong["boot"] = json!("not-a-boot");
        assert!(parse(&wrong).unwrap().validate().is_err());
        assert!(
            parse(&json!([
                good["boot"],
                good["identity"],
                good["lock"],
                good["pid"]
            ]))
            .is_err()
        );
    }
}
