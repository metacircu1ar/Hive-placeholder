//! Indexing regressions.
use super::*;

#[test]
fn directory_barriers_allow_untouched_search_only_ancestors_but_require_modified_parent() {
    let temp = tempfile::tempdir().unwrap();
    let ancestor = temp.path().join("search-only");
    let home = ancestor.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o311)).unwrap();
    let child = home.join("new");
    let result = (|| -> crate::Result<()> {
        files::directory(&child)?;
        files::directory(&child)?;
        files::write(&child.join("record"), b"ok", false)?;
        files::write(&child.join("record"), b"ok", false)
    })();
    // A modification immediately below a write/search-only parent cannot be
    // acknowledged: opening that parent for its mandatory barrier must fail.
    let mandatory = files::directory(&ancestor.join("new-child"));
    let retry = files::directory(&ancestor.join("new-child"));
    let nested = ancestor.join("new-parent/child");
    let nested_first = files::directory(&nested);
    let nested_retry = files::directory(&nested);
    let created_parent = ancestor.join("new-parent").exists();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    result.unwrap();
    if rustix::process::getuid().as_raw() != 0 {
        assert!(mandatory.is_err());
        assert!(retry.is_err());
        assert!(nested_first.is_err());
        assert!(nested_retry.is_err());
        assert!(!created_parent);
    }
}
