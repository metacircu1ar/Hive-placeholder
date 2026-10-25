use super::*;
use crate::disk::Disk;
use std::{os::unix::fs::PermissionsExt, path::Path};

mod safety;

fn fixture() -> (tempfile::TempDir, Disk) {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    (root, disk)
}

fn native_pages(root: &Path, prefix: &str) -> Vec<String> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            result.extend(native_pages(&entry.path(), &format!("{prefix}{name}/")));
        } else if let Some(stem) = name.strip_suffix(".md") {
            result.push(format!("{prefix}{stem}"));
        }
    }
    result
}

#[test]
fn nested_pages_keep_depth_first_native_directory_order() {
    let (root, disk) = fixture();
    for name in [
        "zebra",
        "alpha",
        "middle",
        "nested/z",
        "other/deep/end",
        "nested/a",
    ] {
        let slug = name.parse().unwrap();
        let path = disk.page_path(&slug, true).unwrap();
        fs::write(path, name).unwrap();
    }
    fs::write(root.path().join("ignore.txt"), "ignored").unwrap();
    let expected = native_pages(root.path(), "");
    let actual = disk
        .pages()
        .map(|item| {
            let (slug, raw) = item.unwrap();
            let raw = raw.unwrap();
            assert_eq!(slug.as_str(), raw);
            slug.to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), 6);
    assert_eq!(actual, expected);
}

#[test]
fn later_contents_are_loaded_only_when_requested() {
    let (root, disk) = fixture();
    for name in ["first", "second"] {
        fs::write(root.path().join(format!("{name}.md")), "before").unwrap();
    }
    let order = native_pages(root.path(), "");
    let mut pages = disk.pages();
    assert_eq!(pages.next().unwrap().unwrap().0.as_str(), order[0]);
    fs::write(root.path().join(format!("{}.md", order[1])), "after").unwrap();
    let (slug, raw) = pages.next().unwrap().unwrap();
    assert_eq!(slug.as_str(), order[1]);
    assert_eq!(raw.unwrap(), "after");
    assert!(pages.next().is_none());
    assert!(pages.next().is_none());
}

#[test]
fn removing_a_later_file_never_returns_prefetched_contents() {
    let (root, disk) = fixture();
    for name in ["first", "second"] {
        fs::write(root.path().join(format!("{name}.md")), "before").unwrap();
    }
    let order = native_pages(root.path(), "");
    let mut pages = disk.pages();
    assert_eq!(pages.next().unwrap().unwrap().0.as_str(), order[0]);
    fs::remove_file(root.path().join(format!("{}.md", order[1]))).unwrap();
    // ReadDir may retain a directory entry whose name has just disappeared.
    if let Some(item) = pages.next() {
        assert_eq!(item.unwrap_err().code, crate::ErrorCode::IoError);
    }
    assert!(pages.next().is_none());
}

#[test]
fn first_error_fuses_even_when_the_unsafe_file_is_repaired() {
    let (root, disk) = fixture();
    for name in ["first", "second", "third"] {
        fs::write(root.path().join(format!("{name}.md")), "content").unwrap();
    }
    let order = native_pages(root.path(), "");
    let mut pages = disk.pages();
    assert_eq!(pages.next().unwrap().unwrap().0.as_str(), order[0]);
    let next = root.path().join(format!("{}.md", order[1]));
    fs::set_permissions(&next, fs::Permissions::from_mode(0o666)).unwrap();
    let error = pages.next().unwrap().unwrap_err();
    assert_eq!(error.message, Error::unsafe_file().message);
    assert!(!error.file_committed);
    assert!(error.request_id.is_none());
    fs::set_permissions(next, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(pages.next().is_none());
    assert!(pages.next().is_none());
    assert_eq!(disk.pages().count(), 3);
}

#[test]
fn iterator_owns_its_root_and_defers_opening_until_first_next() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("comb");
    let mut pages = {
        let disk = Disk::acquire(&root, true).unwrap();
        fs::write(root.join("page.md"), "content").unwrap();
        disk.pages()
    };
    fs::rename(&root, parent.path().join("moved")).unwrap();
    assert_eq!(
        pages.next().unwrap().unwrap_err().code,
        crate::ErrorCode::IoError
    );
    fs::create_dir(&root).unwrap();
    fs::write(root.join("page.md"), "replacement").unwrap();
    assert!(pages.next().is_none());
}

#[test]
fn early_consumer_error_does_not_advance_to_a_later_unsafe_page() {
    let (root, disk) = fixture();
    for name in ["first", "second"] {
        fs::write(root.path().join(format!("{name}.md")), "content").unwrap();
    }
    let order = native_pages(root.path(), "");
    fs::set_permissions(
        root.path().join(format!("{}.md", order[1])),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let mut pages = disk.pages();
    let mut visited = Vec::new();
    let result = pages.try_for_each(|item| -> Result<()> {
        let (slug, _) = item?;
        visited.push(slug.to_string());
        Err(Error::frontmatter_missing())
    });
    assert_eq!(
        result.unwrap_err().message,
        Error::frontmatter_missing().message
    );
    assert_eq!(visited, vec![order[0].clone()]);
    assert_eq!(
        pages.next().unwrap().unwrap_err().message,
        Error::unsafe_file().message
    );
}
