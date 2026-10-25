//! Shared by the build script and release unit tests.
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

pub struct Identity {
    pub commit: String,
    pub watch: Vec<PathBuf>,
}

pub fn inspect(root: &Path) -> io::Result<Option<Identity>> {
    if !root.join(".git").try_exists()? {
        return Ok(None);
    }
    let commit = git(root, &["rev-parse", "--verify", "HEAD"])?;
    if !matches!(commit.len(), 40 | 64)
        || !commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(io::Error::other("Git returned an invalid commit identity"));
    }
    let mut watch = Vec::new();
    for name in ["HEAD", "refs", "packed-refs"] {
        let path = PathBuf::from(git(root, &["rev-parse", "--git-path", name])?);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        if path.try_exists()? {
            watch.push(path);
        }
    }
    Ok(Some(Identity { commit, watch }))
}

fn git(root: &Path, args: &[&str]) -> io::Result<String> {
    let mut command = Command::new("git");
    command
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(root);
    // A caller's GIT_DIR/GIT_WORK_TREE/config overrides must not identify some
    // unrelated checkout. Never include environment values in diagnostics.
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|name| name.starts_with("GIT_")) {
            command.env_remove(name);
        }
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Git identity command failed ({}, stderr {} bytes)",
            output.status,
            output.stderr.len()
        )));
    }
    let value = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let value = value.trim_end_matches('\n');
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(io::Error::other("Invalid Git identity response"));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(root: &Path, args: &[&str]) {
        let mut command = Command::new("git");
        command.args(args).current_dir(root);
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|name| name.starts_with("GIT_")) {
                command.env_remove(name);
            }
        }
        let output = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "Git fixture failed");
    }

    #[test]
    fn archives_do_not_inherit_an_enclosing_repository() {
        let temp = tempfile::tempdir().unwrap();
        checked(temp.path(), &["init", "-q"]);
        let archive = temp.path().join("archive");
        std::fs::create_dir(&archive).unwrap();
        assert!(inspect(&archive).unwrap().is_none());
    }

    #[test]
    fn checkout_identity_tracks_commits_and_worktree_git_paths() {
        let temp = tempfile::tempdir().unwrap();
        checked(temp.path(), &["init", "-q"]);
        checked(
            temp.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        let first = inspect(temp.path()).unwrap().unwrap();
        assert_eq!(first.commit.len(), 40);
        assert!(first.watch.iter().any(|p| p.ends_with("HEAD")));
        checked(
            temp.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "two",
            ],
        );
        let second = inspect(temp.path()).unwrap().unwrap();
        assert_ne!(first.commit, second.commit);
        let other = temp.path().join("linked");
        checked(
            temp.path(),
            &["worktree", "add", "-q", "--detach", other.to_str().unwrap()],
        );
        assert_eq!(inspect(&other).unwrap().unwrap().commit, second.commit);
    }
}
