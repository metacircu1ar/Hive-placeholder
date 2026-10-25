//! Inspect installed product parts without trusting their output or waiting forever.
use super::{Cli, Report, Status};
use super::{api, details};
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Deserialize, serde::Serialize)]
struct Identity {
    product: String,
    product_version: String,
    compiled_features: Vec<String>,
}

pub(super) fn installed(cli: &Cli, report: &mut Report, required: bool) {
    let sibling = crate::observability::optional(crate::executable::path(), "doctor_executable")
        .and_then(Path::parent)
        .map(|p| p.join("comb"));
    let binary = sibling.filter(|p| p.is_file()).or_else(|| {
        crate::observability::optional(
            crate::process::executable("comb".as_ref()),
            "doctor_optional_comb",
        )
    });
    let Some(binary) = binary else {
        report.add(
            "comb_binary",
            if required {
                Status::Error
            } else {
                Status::Info
            },
            api::value("Comb is not installed."),
            if required {
                vec!["hive-install --comb".into()]
            } else {
                vec![]
            },
        );
        return;
    };
    let result = probe(&binary, Duration::from_secs(cli.probe_timeout))
        .and_then(|bytes| serde_json::from_slice::<Identity>(&bytes).map_err(std::io::Error::other))
        .inspect_err(|error| {
            crate::observability::observation_failed("doctor_comb_identity", error);
        });
    match result {
        Ok(identity) if identity.product == "Comb" => report.add(
            "comb_binary",
            Status::Ok,
            api::value(details::Installed {
                path: &binary,
                identity: &identity,
            }),
            vec![],
        ),
        Ok(_) | Err(_) => report.add(
            "comb_binary",
            Status::Error,
            api::value(details::ProbeFailure {
                path: &binary,
                message: "Cannot read structured Comb version within the probe deadline.",
            }),
            vec![format!(
                "{} version --json",
                super::quote(&binary.to_string_lossy())
            )],
        ),
    }
}

pub(super) fn skills(report: &mut Report) {
    let base = crate::observability::optional(
        super::super::setup::parts::skills(None),
        "doctor_skills_directory",
    )
    .unwrap_or_else(|| PathBuf::from("/missing-installation/share/hive/skills"));
    let hive = base.join("hive/SKILL.md");
    let curator = base.join("comb-curator/SKILL.md");
    report.add(
        "skills",
        if hive.is_file() && curator.is_file() {
            Status::Ok
        } else {
            Status::Warning
        },
        api::value(details::Skills {
            hive,
            comb_curator: curator,
        }),
        vec![],
    );
    match super::super::setup::skills::stale(&base) {
        Ok(paths) if paths.is_empty() => (),
        Ok(paths) => report.add(
            "user_skills",
            Status::Warning,
            api::value(details::StaleSkills { stale: paths }),
            vec![
                "hivectl setup --user-skills codex,claude (select the providers whose owned \
                copies you want to update)"
                    .into(),
            ],
        ),
        Err(error) => report.add(
            "user_skills",
            Status::Warning,
            super::api::value(error),
            vec![],
        ),
    }
}

/// The leader remains unreaped until its entire process group is signalled, so
/// its PID cannot be reused while cleanup targets the group. No threads can hang
/// on a descendant's inherited pipe; capture is nonblocking and byte bounded.
struct Probe(Child, bool);
impl Probe {
    fn signal(&self) {
        let result = crate::os::has_exited(&self.0)
            .and_then(|_| crate::os::signal_group(&self.0, crate::os::Signal::KILL));
        if let Err(error) = result {
            if error.raw_os_error() == Some(libc::ESRCH) {
                tracing::debug!(name: "doctor.probe_already_exited", pid = self.0.id());
            } else {
                tracing::warn!(name: "doctor.probe_cleanup_failed",
                     pid = self.0.id(),
                     phase = "signal",
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "preserve_probe_result");
            }
        }
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        self.signal();
        if let Err(error) = self.0.wait() {
            tracing::warn!(name: "doctor.probe_cleanup_failed",
                 pid = self.0.id(),
                 phase = "wait",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "preserve_probe_result");
        }
    }
}
fn probe(binary: &Path, timeout: Duration) -> std::io::Result<Vec<u8>> {
    let mut command = Command::new(binary);
    command
        .args(["version", "--json"])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = Probe(crate::os::spawn(&mut command)?, false);
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("Missing version pipe"))?;
    crate::os::nonblocking(&stdout)?;
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        let mut buffer = [0; 4096];
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if bytes.len() + n > 65536 {
                    return Err(std::io::Error::other("Version output exceeds 64 KiB"));
                }
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(PROBE_POLL);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    // EOF is not an exit; waitid observes without reaping, preserving group ownership.
    while !crate::process::has_exited(&child.0).map_err(std::io::Error::other)? {
        if Instant::now() >= deadline {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(PROBE_POLL);
    }
    // Kill remaining pipe-less children before reaping the leader.
    child.signal();
    let status = child.0.wait()?;
    // Drop must not signal a PID after wait reaped it.
    child.1 = true;
    if !status.success() {
        return Err(std::io::Error::other("Version probe failed"));
    }
    Ok(bytes)
}

const PROBE_POLL: std::time::Duration = Duration::from_millis(10);

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn version_probe_bounds_output_and_children_that_hold_pipes_open() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("comb");
        let write = |body: &str| {
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        write("printf '{\"product\":\"Comb\"}'");
        // A fresh script's first exec can take seconds under load on macOS; a
        // successful probe returns as soon as the script exits.
        assert_eq!(
            probe(&script, Duration::from_secs(10)).unwrap(),
            b"{\"product\":\"Comb\"}"
        );
        write("sleep 30 &\nexit 0");
        let start = Instant::now();
        assert_eq!(
            probe(&script, Duration::from_millis(100))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        write("while :; do printf 'too much output'; done");
        assert!(
            probe(&script, Duration::from_secs(10))
                .unwrap_err()
                .to_string()
                .contains("64 KiB")
        );
        write("printf '{}'\nexit 1");
        assert!(probe(&script, Duration::from_secs(2)).is_err());
    }
}
