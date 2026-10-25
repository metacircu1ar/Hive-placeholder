//! Admit the exact current marker before ordinary state writes. Empty roots only
//! may initialize; an existing markerless development root is never adopted.
use super::{State, check_path, read_json};
use crate::{
    compatibility,
    error::{Error, Result},
};
use hive_input::compatibility::Marker;
use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};

const BYTES: &[u8] = b"{\"format\":1}\n";

pub(super) fn check(root: &Path) -> Result<bool> {
    if super::recovery::exists(&root.join(hive_setup_support::migration::PENDING))? {
        return Err(Error::recovery_required(
            "An interrupted state upgrade must be recovered before normal operations; \
                run hivectl state upgrade with its matching build.",
        ));
    }
    marker(root)
}

pub(super) fn marker(root: &Path) -> Result<bool> {
    if !super::recovery::exists(root)? {
        return Ok(false);
    }
    check_path(root, true)?;
    let path = root.join("format");
    if !super::recovery::exists(&path)? {
        if fs::read_dir(root)
            .map_err(Error::state_io_from)?
            .next()
            .transpose()
            .map_err(Error::state_io_from)?
            .is_none()
        {
            return Ok(false);
        }
        return Err(unsupported());
    }
    let marker: Marker = read_json(&path)?;
    if marker.format > compatibility::STATE.current {
        return Err(compatibility::newer());
    }
    if marker.format != compatibility::STATE.current {
        return Err(unsupported());
    }
    Ok(true)
}

fn unsupported() -> Error {
    Error::unsupported_schema(
        "Unsupported development state marker. Use a fresh --state-dir; this \
            directory was not modified.",
    )
}

impl State {
    /// Serialize fresh-root publication with temporary-root cleanup. The returned
    /// guard must outlive publication of the caller's first root-owned records.
    pub(crate) fn initialize(&self) -> Result<super::admission::Initialization> {
        self.check_format()?;
        let guard = self.initialization_lock()?;
        if check(&self.root)? {
            super::journal::sync_record(&self.root.join("format"))?;
            super::sync_directory(self.root.parent().ok_or_else(super::unsafe_state)?)
                .map_err(Error::state_io_from)?;
            return Ok(guard);
        }
        if super::recovery::exists(&self.root)? {
            // Only the empty directory admitted above can reach this operation.
            publish_marker(&self.root)?;
        } else {
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .staged_dir(&self.root, |stage| {
                    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                        .create_new(stage.join("format"), BYTES)
                        .map_err(io::Error::other)
                })
                .map_err(Error::state_publication)?;
        }
        check(&self.root)?;
        Ok(guard)
    }

    pub(crate) fn initialization_lock(&self) -> Result<super::admission::Initialization> {
        let admission = self.admission()?;
        let publication = hive_setup_support::locks::path_blocking(&self.root, "hive-state", true)
            .map_err(Error::state_io_from)?;
        Ok(super::admission::Initialization {
            _publication: publication,
            _admission: admission,
        })
    }
}

fn publish_marker(root: &Path) -> Result<()> {
    // A crash may leave this private sibling behind. It never makes the empty
    // target root look like unsupported state, and is never adopted on retry.
    let stage = tempfile::Builder::new()
        .prefix(".hive-tmp-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(root.parent().ok_or_else(super::unsafe_state)?)
        .map_err(Error::state_io_from)?;
    let path = root.join("format");
    let (stage, ()) = crate::publication::prepare(stage, |stage| {
        atomic_fs::PreparedCreate::new_in(
            &path,
            stage,
            hive_setup_support::names::PUBLICATION_OPTIONS,
        )
        .and_then(|prepared| prepared.publish_with(BYTES, &super::journal::Boundary(&path)))
        .map_err(Error::state_publication)
    })?;
    // prepare has checked the directory identity. Successful publication leaves
    // it empty; never recursively remove unexpected entries added afterwards.
    fs::remove_dir(stage.keep()).map_err(Error::state_io_from)
}

pub(super) fn prepare(root: &Path) -> io::Result<()> {
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(root.join("format"), BYTES)
        .map_err(io::Error::other)
}

/// Temporary metadata wrappers are not recursive purge authority.
pub(super) fn remove(root: &Path) -> Result<()> {
    if super::recovery::exists(&root.join("format"))? {
        check(root)?;
        let path = root.join("format");
        fs::remove_file(&path).map_err(Error::state_io_from)?;
        super::journal::Boundary(&path).sync_parent()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_empty_root_initialization_is_retryable_without_adoption() {
        for boundary in ["before", "after"] {
            let temp = tempfile::tempdir().unwrap();
            let state = State::new(temp.path().join("state")).unwrap();
            super::super::directory(&state.root).unwrap();
            let mut command = crate::test_support::rerun_isolated(
                "state::format::tests::marker_initialization_death_fixture",
                true,
            );
            let status = command
                .env("HIVE_MARKER_TEST_ROOT", &state.root)
                .env("HIVE_MARKER_TEST_BOUNDARY", boundary)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(91));
            assert_eq!(
                fs::read_dir(&state.root).unwrap().count(),
                usize::from(boundary == "after")
            );
            drop(state.initialize().unwrap());
            state.check_format().unwrap();
            assert_eq!(fs::read(state.root.join("format")).unwrap(), BYTES);
        }
    }

    #[test]
    #[ignore = "owned process death fixture"]
    fn marker_initialization_death_fixture() {
        let state = State::new(std::env::var_os("HIVE_MARKER_TEST_ROOT").unwrap().into()).unwrap();
        let boundary = if std::env::var("HIVE_MARKER_TEST_BOUNDARY").unwrap() == "before" {
            atomic_fs::Step::BeforePublish
        } else {
            atomic_fs::Step::AfterPublish
        };
        crate::state::sync_tests::with_hook(
            move |path, step| {
                if path.file_name().is_some_and(|name| name == "format") && step == boundary {
                    std::process::exit(91);
                }
                Ok(())
            },
            || drop(state.initialize().unwrap()),
        );
        panic!("marker death boundary was not reached");
    }

    #[test]
    fn current_marker_is_published_only_for_new_or_empty_roots() {
        for existing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = State::new(temp.path().join("state")).unwrap();
            if existing {
                super::super::directory(&state.root).unwrap();
            }
            state.check_format().unwrap();
            assert!(!state.root.join("format").exists());
            drop(state.initialize().unwrap());
            let bytes = fs::read(state.root.join("format")).unwrap();
            let marker: Marker = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(marker, Marker::current(&compatibility::STATE));
            crate::compatibility::contract("state-marker.json", &marker);
            drop(state.initialize().unwrap());
            assert_eq!(fs::read(state.root.join("format")).unwrap(), bytes);
        }
    }

    #[test]
    fn markerless_nonempty_state_is_refused_without_adoption_or_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let state = State::new(temp.path().join("state")).unwrap();
        super::super::directory(&state.root).unwrap();
        let path = state.root.join("index.json");
        let original = b"{\"profiles\":{},\"generations\":{}}";
        super::super::write_json(
            &path,
            &serde_json::from_slice::<serde_json::Value>(original).unwrap(),
        )
        .unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(
            state.check_format().unwrap_err().code(),
            "unsupported_schema"
        );
        assert_eq!(
            state.initialize().err().unwrap().code(),
            "unsupported_schema"
        );
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(!state.root.join("format").exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn future_and_malformed_markers_are_refused_unchanged() {
        for (bytes, code) in [
            (b"{\"format\":2}".as_slice(), "newer_format"),
            (b"{\"format\":0}".as_slice(), "unsupported_schema"),
            (b"[1]".as_slice(), "state_inconsistent"),
            (
                b"{\"format\":1,\"extra\":false}".as_slice(),
                "state_inconsistent",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = State::new(temp.path().join("state")).unwrap();
            drop(state.initialize().unwrap());
            let path = state.root.join("format");
            fs::write(&path, bytes).unwrap();
            assert_eq!(state.check_format().unwrap_err().code(), code);
            assert_eq!(state.initialize().err().unwrap().code(), code);
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(fs::read_dir(&state.root).unwrap().count(), 1);
        }
    }

    #[test]
    fn replaced_marker_stage_and_preparation_survive_failure_and_unwind() {
        use std::cell::RefCell;
        use std::rc::Rc;
        for replace_directory in [false, true] {
            for panic in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let parent = temp.path().to_owned();
                let state = State::new(parent.join("state")).unwrap();
                super::super::directory(&state.root).unwrap();
                let retained = Rc::new(RefCell::new(None));
                let observed = Rc::clone(&retained);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::state::sync_tests::with_hook(
                        move |path, step| {
                            if path.file_name().is_some_and(|name| name == "format")
                                && step == atomic_fs::Step::BeforePublish
                            {
                                let preparation = marker_stage(&parent);
                                let sentinel = if replace_directory {
                                    fs::rename(&preparation, parent.join("moved-stage")).unwrap();
                                    super::super::directory(&preparation).unwrap();
                                    preparation.join("sentinel")
                                } else {
                                    let entry = fs::read_dir(&preparation)
                                        .unwrap()
                                        .next()
                                        .unwrap()
                                        .unwrap()
                                        .path();
                                    fs::rename(&entry, parent.join("moved-preparation")).unwrap();
                                    entry
                                };
                                fs::write(&sentinel, b"unrelated replacement").unwrap();
                                *observed.borrow_mut() = Some(sentinel);
                                assert!(!panic, "owned marker unwind fixture");
                                return Err(io::Error::from_raw_os_error(libc::EIO));
                            }
                            Ok(())
                        },
                        || state.initialize(),
                    )
                }));
                if panic {
                    assert!(result.is_err());
                } else {
                    assert!(result.unwrap().is_err());
                }
                let sentinel = retained.borrow().clone().unwrap();
                assert_eq!(fs::read(&sentinel).unwrap(), b"unrelated replacement");
                assert!(!state.root.join("format").exists());
                drop(state.initialize().unwrap());
                assert_eq!(fs::read(sentinel).unwrap(), b"unrelated replacement");
            }
        }
    }

    #[test]
    fn successful_marker_publication_does_not_recursively_remove_unexpected_stage_entries() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().to_owned();
        let state = State::new(parent.join("state")).unwrap();
        super::super::directory(&state.root).unwrap();
        let result = crate::state::sync_tests::with_hook(
            move |path, step| {
                if path.file_name().is_some_and(|name| name == "format")
                    && step == atomic_fs::Step::AfterDirectorySync
                {
                    fs::write(marker_stage(&parent).join("sentinel"), b"unrelated").unwrap();
                }
                Ok(())
            },
            || state.initialize(),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(state.root.join("format")).unwrap(), BYTES);
        let sentinel = marker_stage(temp.path()).join("sentinel");
        assert_eq!(fs::read(&sentinel).unwrap(), b"unrelated");
        drop(state.initialize().unwrap());
        assert_eq!(fs::read(sentinel).unwrap(), b"unrelated");
    }

    fn marker_stage(parent: &Path) -> std::path::PathBuf {
        fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(".hive-tmp-")
            })
            .unwrap()
    }
}
