//! Lifecycle regressions.
use super::*;

#[test]
fn failed_stop_never_applies_root_changes() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    ctl.start(&|| Ok(mock.ready.get())).unwrap();
    mock.failure.set(true);
    let changed = Cell::new(false);
    assert!(
        ctl.paused(&|| Ok(mock.ready.get()), || {
            changed.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!changed.get());
    assert!(mock.ready.get());
}

#[test]
fn unsupported_untouched_ancestor_does_not_skip_mandatory_parent_barriers() {
    let temp = tempfile::tempdir().unwrap();
    let ancestor = temp.path().canonicalize().unwrap();
    let private = ancestor.join("private");
    fs::create_dir(&private).unwrap();
    for errno in [libc::EINVAL, libc::ENOTSUP, libc::EIO] {
        files::FAIL_DIRECTORY_SYNC
            .with(|fault| *fault.borrow_mut() = Some((ancestor.clone(), errno)));
        let child = private.join(format!("lower-{errno}"));
        let result = files::directory(&child);
        if errno == libc::EIO {
            assert!(result.is_err());
        } else {
            result.unwrap();
            files::directory(&child).unwrap();
            files::write(&child.join("record"), b"ok", false).unwrap();
        }
        let direct = ancestor.join(format!("direct-{errno}/child"));
        assert!(files::directory(&direct).is_err());
        assert!(files::directory(&direct).is_err());
        assert!(!direct.parent().unwrap().exists());
        files::FAIL_DIRECTORY_SYNC.with(|fault| *fault.borrow_mut() = None);
    }
}
