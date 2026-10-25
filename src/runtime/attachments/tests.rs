use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = crate::runtime::test_directory();
    let private = dir.path().join("state");
    crate::state::directory(&private).unwrap();
    drop(
        crate::state::State::new(private.clone())
            .unwrap()
            .initialize()
            .unwrap(),
    );
    let root = private.join("attachments");
    (dir, private, root)
}
#[test]
fn snapshots_keep_exact_names_deduplicate_and_outlive_sources() {
    let (dir, private, root) = setup();
    let a = dir.path().join("Report.PNG");
    let other = dir.path().join("other");
    fs::create_dir(&other).unwrap();
    let b = other.join("report.PNG");
    fs::write(&a, b"plain text").unwrap();
    fs::copy(&a, &b).unwrap();
    fs::set_permissions(&a, fs::Permissions::from_mode(0o666)).unwrap();
    let link = dir.path().join("original name.txt");
    symlink(&a, &link).unwrap();
    let mut capture = Capture::new(root.clone(), private.clone()).unwrap();
    let items = capture.message(&[a.clone(), a.clone(), b, link]).unwrap();
    assert_eq!(items[0].key(), items[1].key());
    assert_ne!(items[0].key(), items[2].key());
    assert_eq!(items[3].filename, "original name.txt");
    assert_eq!(items[0].media_type, "text/plain");
    capture.publish().unwrap();
    fs::remove_file(&a).unwrap();
    let Source::Snapshot { path, .. } = &items[0].source else {
        panic!()
    };
    assert_eq!(fs::read(path).unwrap(), b"plain text");
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let mut duplicate = Capture::new(root, private).unwrap();
    let copy = dir.path().join("Report.PNG");
    fs::write(&copy, b"plain text").unwrap();
    assert_eq!(duplicate.message(&[copy]).unwrap()[0].key(), items[0].key());
    duplicate.publish().unwrap();
}
#[test]
fn refuses_private_aliases_owned_hardlinks_nonfiles_and_excess_count() {
    let (dir, private, root) = setup();
    let secret = private.join("auth.json");
    fs::write(&secret, b"secret").unwrap();
    let alias = dir.path().join("alias");
    symlink(&secret, &alias).unwrap();
    let hard = dir.path().join("hard");
    fs::hard_link(&secret, &hard).unwrap();
    let socket = dir.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut capture = Capture::new(root, private).unwrap();
    for path in [
        secret,
        alias,
        hard,
        socket,
        dir.path().to_owned(),
        PathBuf::from("relative"),
    ] {
        let e = capture.message(std::slice::from_ref(&path)).unwrap_err();
        assert_eq!(e.code(), "attachment_invalid");
        assert!(e.message().contains(path.to_string_lossy().as_ref()));
    }
    assert!(
        capture
            .message(&vec![dir.path().join("missing"); MAX_FILES + 1])
            .is_err()
    );
}
#[test]
fn file_and_cumulative_limits_preserve_all_live_references() {
    let (dir, private, root) = setup();
    let mut paths = Vec::new();
    for (index, size) in [FILE_BYTES + 1, FILE_BYTES, FILE_BYTES, FILE_BYTES, 1]
        .into_iter()
        .enumerate()
    {
        let path = dir.path().join(format!("{index}.log"));
        fs::File::create(&path).unwrap().set_len(size).unwrap();
        paths.push(path);
    }
    let mut capture = Capture::new(root, private).unwrap();
    let items = capture.message(&paths).unwrap();
    assert!(matches!(
        items[0].source,
        Source::LivePath {
            reason: LiveReason::FileSizeLimit,
            ..
        }
    ));
    assert!(items[1].key().is_some());
    assert!(items[2].key().is_some());
    for item in &items[3..] {
        assert!(matches!(
            item.source,
            Source::LivePath {
                reason: LiveReason::MessageSizeLimit,
                ..
            }
        ));
    }
    capture.publish().unwrap();
}
#[test]
fn changed_open_descriptor_is_rejected_and_failed_capture_publishes_nothing() {
    let (dir, private, root) = setup();
    let path = dir.path().join("source");
    fs::write(&path, b"before").unwrap();
    let source = files::SourceFile::open(&path, &private.canonicalize().unwrap()).unwrap();
    fs::write(&path, b"after").unwrap();
    assert!(source.unchanged().is_err());
    let mut capture = Capture::new(root.clone(), private).unwrap();
    assert!(
        capture
            .message(&[path, dir.path().join("missing")])
            .is_err()
    );
    drop(capture);
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}
#[test]
fn media_uses_magic_instead_of_extension_and_bounds_header_reads() {
    let (dir, private, root) = setup();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(2000_u32.to_be_bytes());
    png.extend(1000_u32.to_be_bytes());
    let path = dir.path().join("actually an image.log");
    fs::write(&path, png).unwrap();
    let item = Capture::new(root, private)
        .unwrap()
        .message(&[path])
        .unwrap()
        .remove(0);
    assert_eq!(item.media_type, "image/png");
    assert_eq!(item.dimensions, Some([2000, 1000]));
    assert_eq!(
        media::inspect(b"GIF89a\x01\0\x02\0", false),
        ("image/gif", Some([1, 2]))
    );
    assert_eq!(media::inspect(b"\xff\xd8\xff", false), ("image/jpeg", None));
}

#[test]
fn utf8_text_prefix_can_end_inside_a_character_but_complete_invalid_bytes_are_binary() {
    let (dir, private, root) = setup();
    let text = dir.path().join("cyrillic.log");
    fs::write(&text, format!("x{}", "я".repeat(32769))).unwrap();
    let mut capture = Capture::new(root, private).unwrap();
    assert_eq!(
        capture.message(&[text]).unwrap()[0].media_type,
        "text/plain"
    );
    assert_eq!(
        media::inspect(b"text\xff", true).0,
        "application/octet-stream"
    );
    assert_eq!(
        media::inspect(b"text\xd0", false).0,
        "application/octet-stream"
    );
}

#[test]
fn state_preparation_basename_is_legal_in_an_attachment_key_directory() {
    let (dir, private, root) = setup();
    let source = dir.path().join(".hive-tmp-state-ABC123");
    fs::write(&source, b"keep attachment").unwrap();
    let mut capture = Capture::new(root.clone(), private.clone()).unwrap();
    let items = capture.message(&[source]).unwrap();
    capture.publish().unwrap();
    items[0].validate(&root.canonicalize().unwrap()).unwrap();
    let Source::Snapshot { path, .. } = &items[0].source else {
        panic!()
    };
    crate::state::directory(&private.join("runtime")).unwrap();
    let lock = crate::state::Lock::acquire(&private.join("runtime/daemon.lock")).unwrap();
    crate::state::State::new(private)
        .unwrap()
        .cleanup_preparations(&lock)
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"keep attachment");
    assert_eq!(items[0].filename, ".hive-tmp-state-ABC123");
}
