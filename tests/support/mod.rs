#![allow(dead_code)] // Shared helpers are used by different integration-test binaries.

pub mod process;
pub mod wait;
#[allow(unused_imports)] // Each integration binary uses its own subset of guards.
pub use process::{ChildGuard, PermissionsGuard, ReleaseFile, ThreadGuard, command};

use std::{
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// Copied providers use this self-contained helper, never the maintainer checkout.
pub fn mock_files(root: &Path) {
    std::fs::write(
        root.join("codex-schema.json"),
        include_str!("../fixtures/codex-schema.json"),
    )
    .expect("copy offline provider schema fixture");
    std::fs::write(
        root.join("mock_files.py"),
        include_str!("../../scripts/release/files.py"),
    )
    .expect("copy atomic mock filesystem helper");
}

pub fn publish_fixture(path: &Path, bytes: &[u8]) {
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .write(path, bytes)
    .expect("publish complete shared fixture");
}

pub fn python() -> &'static Path {
    static PYTHON: OnceLock<PathBuf> = OnceLock::new();
    PYTHON.get_or_init(|| {
        let output = command("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .expect("locate the fixture's Python interpreter");
        assert!(output.status.success(), "python3 could not start");
        let path = PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("Python executable path is UTF-8")
                .trim(),
        );
        assert!(path.is_absolute(), "Python executable must be absolute");
        path
    })
}

/// Avoid an extra /usr/bin/env exec inside the provider's timeout. macOS can
/// delay concurrent executable loads by seconds, before the fixture even runs.
/// A short, space-free link also supports interpreters installed in long paths
/// or paths containing spaces, which cannot be used directly in a shebang.
pub fn python_shebang(root: &Path) -> String {
    let link = root.join("python3");
    symlink(python(), &link).expect("link the fixture's Python interpreter");
    let path = link.to_str().expect("fixture path is UTF-8");
    assert!(path.len() < 120 && !path.contains(char::is_whitespace));
    format!("#!{path}\n")
}
