//! Requests regressions.
use super::*;

#[test]
fn obsolete_or_malformed_staging_names_cannot_bypass_ownership_validation() {
    for name in [
        ".tmpABC123",
        ".hive-tmp-ABCDE",
        ".hive-tmp-0123456789abcdeF",
    ] {
        for relative in ["", "writers"] {
            let (_tmp, state, binding) = fixture();
            let _fork = crate::os::fork_guard().unwrap();
            let dir = binding.directory.join(relative);
            directory(&dir).unwrap();
            let path = dir.join(name);
            crate::state::private_file(&path, true)
                .unwrap()
                .write_all(b"partial")
                .unwrap();
            assert!(state.runtime_lease(&binding).is_err(), "{relative}/{name}");
            assert_eq!(fs::read(path).unwrap(), b"partial");
        }
    }
}
