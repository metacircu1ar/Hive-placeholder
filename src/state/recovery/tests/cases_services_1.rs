//! Services regressions.

#[test]
fn claude_registration_without_user_environment_finishes() {
    use std::{io::Read, process::Stdio, time::Duration};

    for user in [None, Some("")] {
        for test in [
            "claude_namespace_is_normalized_and_keychain_account_stays_pinned",
            "claude_index_rejects_missing_pinned_keychain_account",
        ] {
            let name = format!("state::recovery::tests::{test}");
            let mut command = crate::test_support::rerun_isolated(&name, false);
            if let Some(user) = user {
                command.env("USER", user);
            } else {
                command.env_remove("USER");
            }
            let mut child = crate::test_support::ChildGuard(
                crate::os::spawn(command.stdout(Stdio::piped())).unwrap(),
            );
            // Wait before reading: a recursive fork lock must fail within a
            // deadline, with ChildGuard still owning kill/reap on timeout.
            let status = crate::test_support::wait_until(
                Duration::from_secs(20),
                Duration::from_millis(10),
                || child.try_wait().unwrap(),
            );
            let mut output = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .take(64 * 1024)
                .read_to_string(&mut output)
                .unwrap();
            assert!(
                status.success()
                    && output.lines().any(|line| line.starts_with(
                        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
                    )),
                "{name} with USER={user:?}: {status}; {output}"
            );
        }
    }
}
