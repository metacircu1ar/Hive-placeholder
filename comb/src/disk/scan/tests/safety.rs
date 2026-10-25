use super::*;
use std::{
    ffi::OsString,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, symlink},
    },
};

fn only_error(disk: &Disk) -> Error {
    let mut pages = disk.pages();
    let error = pages.next().unwrap().unwrap_err();
    assert!(pages.next().is_none());
    error
}

#[test]
fn hidden_names_skip_type_and_device_checks() {
    let (root, mut disk) = fixture();
    symlink("missing", root.path().join(".hidden")).unwrap();
    disk.root_device ^= 1;
    assert!(disk.pages().next().is_none());
}

#[test]
fn hidden_non_utf8_names_fail_conversion() {
    // APFS rejects these bytes at file creation, so exercise the scanner's
    // conversion directly; it runs before the hidden-name skip on every OS.
    let invalid = OsString::from_vec(b".hidden-\xff".to_vec());
    assert_eq!(
        entry_name(invalid).unwrap_err().message,
        Error::page_path_not_utf8().message
    );
}

#[test]
fn visible_non_markdown_symlinks_fail_before_device_checks() {
    let (root, mut disk) = fixture();
    symlink("missing", root.path().join("notes.txt")).unwrap();
    disk.root_device ^= 1;
    assert_eq!(only_error(&disk).message, Error::scan_symlink().message);
}

#[test]
fn non_markdown_entries_still_check_device_before_ignoring_contents() {
    let (root, mut disk) = fixture();
    let path = root.path().join("notes.txt");
    fs::write(&path, "ignored").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(disk.pages().next().is_none());
    disk.root_device ^= 1;
    assert_eq!(
        only_error(&disk).message,
        Error::cross_device_comb().message
    );
}

#[test]
fn slug_validation_precedes_opening_and_file_permissions() {
    let (root, disk) = fixture();
    let path = root.path().join("UPPER.md");
    fs::write(&path, "content").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(only_error(&disk).message, Error::invalid_slug().message);
}

#[test]
fn directory_device_validation_precedes_its_mode_check() {
    let (root, mut disk) = fixture();
    let directory = root.path().join("nested");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(only_error(&disk).message, Error::unsafe_directory().message);
    disk.root_device ^= 1;
    assert_eq!(
        only_error(&disk).message,
        Error::cross_device_comb().message
    );
}

#[test]
fn hard_linked_markdown_remains_unsafe() {
    let (root, disk) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let source = outside.path().join("source.md");
    fs::write(&source, "content").unwrap();
    fs::hard_link(source, root.path().join("page.md")).unwrap();
    assert_eq!(only_error(&disk).message, Error::unsafe_file().message);
}

#[test]
fn fifo_markdown_is_rejected_without_waiting_for_a_writer() {
    use std::os::unix::fs::FileTypeExt;
    let (root, disk) = fixture();
    let path = root.path().join("page.md");
    crate::test_support::fifo(&path).unwrap();
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_fifo());
    let start = std::time::Instant::now();
    assert_eq!(only_error(&disk).message, Error::unsafe_file().message);
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn cached_device_is_used_by_pages_paths_heads_and_receipts() {
    let (root, mut disk) = fixture();
    let slug = "page".parse().unwrap();
    fs::write(root.path().join("page.md"), "content").unwrap();
    assert_eq!(disk.root_device, fs::metadata(root.path()).unwrap().dev());
    disk.root_device ^= 1;
    let expected = Error::cross_device_comb().message;
    assert_eq!(only_error(&disk).message, expected);
    assert_eq!(disk.page_path(&slug, false).unwrap_err().message, expected);
    assert_eq!(disk.head(&slug).unwrap_err().message, expected);
    assert_eq!(disk.set_head(&slug, None).unwrap_err().message, expected);
    assert_eq!(
        disk.receipt_path("human", "request").unwrap_err().message,
        expected
    );
}

#[test]
fn case_insensitive_paths_still_require_the_stored_spelling() {
    let (root, disk) = fixture();
    let actual = root.path().join("Stored.md");
    fs::write(actual, "content").unwrap();
    let requested = root.path().join("stored.md");
    if requested.exists() {
        assert_eq!(
            read(&requested, 1024).unwrap_err().message,
            Error::path_spelling_mismatch().message
        );
    } else {
        assert!(read(&requested, 1024).unwrap_err().is_missing_file());
    }
    assert_eq!(only_error(&disk).message, Error::invalid_slug().message);
}
