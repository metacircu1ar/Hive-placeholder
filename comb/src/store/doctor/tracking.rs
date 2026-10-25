//! A missing Git check is incomplete evidence, never proof of untracked state.
use crate::store::{Finding, FindingKind};
use hive_setup_support::runner::{Output, capture};
use std::{io, path::Path, time::Duration};

pub(super) fn check(root: &Path) -> io::Result<Output> {
    let root = hive_setup_support::service::path_text(root)?;
    capture(
        std::process::Command::spawn,
        "git",
        &[
            "-C".into(),
            root,
            "-c".into(),
            "core.fsmonitor=false".into(),
            "ls-files".into(),
            "--error-unmatch".into(),
            "--".into(),
            ".comb".into(),
        ],
        TRACKING_TIMEOUT,
    )
}

pub(super) fn finding(result: io::Result<Output>) -> Option<Finding> {
    let kind = match result {
        Ok(output) if output.code == 0 => FindingKind::PrivateStateTracked,
        // Git's documented unmatched-path status; other exits cannot establish safety.
        Ok(output) if output.code == 1 => return None,
        Ok(output) => {
            tracing::warn!(name: "comb.doctor_check_unavailable",
                 phase = "git_tracking",
                 exit_code = output.code,
                 decision = "report_incomplete_check");
            FindingKind::PrivateStateUnchecked
        }
        Err(error) => {
            tracing::warn!(name: "comb.doctor_check_unavailable",
                 phase = "git_tracking",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "report_incomplete_check");
            FindingKind::PrivateStateUnchecked
        }
    };
    let detail = if kind == FindingKind::PrivateStateTracked {
        "Private Comb administration files are tracked by Git. Remove them from \
            tracking and treat any published seal key as exposed."
    } else {
        "Git tracking could not be checked. Ensure Git is available and this root \
            is in a Git repository, then rerun doctor; private state has not been \
            verified as untracked."
    };
    Some(Finding {
        slug: ".comb".into(),
        kind,
        detail: detail.into(),
    })
}

const TRACKING_TIMEOUT: std::time::Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_and_failed_checks_never_claim_safety_or_echo_native_output() {
        for code in [0, 1, 128, -1] {
            let result = finding(Ok(Output {
                code,
                stdout: "private-account-path".into(),
                stderr: "secret-token".into(),
            }));
            if code == 1 {
                assert!(result.is_none());
            } else {
                let result = result.unwrap();
                assert_eq!(
                    result.kind,
                    if code == 0 {
                        FindingKind::PrivateStateTracked
                    } else {
                        FindingKind::PrivateStateUnchecked
                    }
                );
                assert!(!result.detail.contains("private-account-path"));
                assert!(!result.detail.contains("secret-token"));
            }
        }
        let result = finding(Err(io::Error::from_raw_os_error(libc::ENOENT))).unwrap();
        assert_eq!(result.kind, FindingKind::PrivateStateUnchecked);
    }
}
