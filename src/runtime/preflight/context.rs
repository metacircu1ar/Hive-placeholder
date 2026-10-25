//! Spawn context distinguishes caller paths/content from saved or configured state.
use super::fs_context::FsContext;
use crate::{
    error::{Error, Result},
    runtime::{
        api::{InputContext as Op, SpawnArgs},
        branching::Source,
        core::{INSTRUCTION_SNAPSHOT_BYTES, text_file},
        invalid_request,
    },
};
use hive_input::{Code, validate};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(super) fn directory(path: &Path, explicit: bool) -> Result<PathBuf> {
    path_shape(path, explicit)?;
    let metadata = std::fs::metadata(path).map_err(|cause| {
        if explicit {
            directory_io(cause)
        } else {
            inherited_directory().with_source(cause)
        }
    })?;
    if !metadata.is_dir() {
        return Err(if explicit {
            Op::Spawn.error(
                Code::InvalidPath,
                "/cwd",
                "an existing accessible directory",
            )
        } else {
            inherited_directory()
        });
    }
    let canonical = path.canonicalize().map_err(|cause| {
        if explicit {
            directory_io(cause)
        } else {
            Error::state_io_from(cause)
        }
    })?;
    path_shape(&canonical, explicit)?;
    Ok(canonical)
}

fn path_shape(path: &Path, explicit: bool) -> Result<()> {
    validate::absolute_path(path).map_err(|rule| {
        if explicit {
            Op::Spawn
                .error(rule.code(), "/cwd", rule.expected())
                .with_source(rule)
        } else {
            inherited_directory().with_source(rule)
        }
    })
}

fn inherited_directory() -> Error {
    Error::invalid_cwd("Agent working directory must be an existing absolute directory.")
}

fn directory_io(cause: io::Error) -> Error {
    if crate::os::input_path_error(&cause) {
        Op::Spawn
            .error(
                Code::InvalidPath,
                "/cwd",
                "an existing accessible directory",
            )
            .with_source(cause)
    } else {
        Error::state_io_from(cause)
    }
}

pub(super) fn instructions(
    fs: &FsContext,
    args: &SpawnArgs,
    resolved: &crate::config::ResolvedSpawn,
    source: Option<&Source>,
) -> Result<String> {
    let inherited = source.filter(|_| {
        args.role.is_missing() && args.preset.is_missing() && args.instructions.is_missing()
    });
    let mut text = inherited.map_or_else(String::new, |s| s.instructions.clone());
    if inherited.is_none() {
        // Preserve sequential reads and the first failing configured file, even
        // if earlier content already exceeds the aggregate cap.
        for path in fs.config.context.iter().chain(
            resolved
                .instructions
                .iter()
                .filter(|_| args.instructions.is_missing()),
        ) {
            text.push_str(&text_file(&fs.config_dir.join(path))?);
            text.push_str("\n\n");
        }
    }
    if text.len() > INSTRUCTION_SNAPSHOT_BYTES {
        return Err(invalid_request());
    }
    if let Some(extra) = args.instructions.as_str() {
        text.push_str(extra);
        validate::byte_count(text.len(), INSTRUCTION_SNAPSHOT_BYTES).map_err(|rule| {
            Op::Spawn
                .error(
                    rule.code(),
                    "/instructions",
                    "instructions fitting within a complete snapshot of at most 262144 UTF-8 \
                        bytes",
                )
                .with_source(rule)
        })?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    #[test]
    fn resolved_path_shape_retains_provenance_and_never_echoes_bytes() -> serde_json::Result<()> {
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/private-canary/\xff"));
        for (explicit, code) in [(true, "invalid_path"), (false, "invalid_cwd")] {
            let error = path_shape(path, explicit).unwrap_err();
            assert_eq!(error.code(), code);
            let json = serde_json::to_string(&error)?;
            assert!(!json.contains("private-canary"));
        }
        Ok(())
    }
    #[test]
    fn path_selection_errors_preserve_native_resource_errors_and_causes() {
        for (errno, code) in [
            (libc::ENOENT, "invalid_path"),
            (libc::EACCES, "invalid_path"),
            (libc::EIO, "state_io"),
            (libc::EMFILE, "state_io"),
        ] {
            let error = directory_io(io::Error::from_raw_os_error(errno));
            assert_eq!(error.code(), code);
            assert_eq!(error.io_raw_os_error(), Some(errno));
        }
    }

    #[test]
    fn oversized_inherited_snapshot_remains_operational_and_can_be_overridden() -> Result<()> {
        let temp = crate::runtime::test_directory();
        let core = crate::runtime::test_support::CoreBuilder::new(temp.path()).build();
        let mut fs = core.fs_context();
        fs.config = crate::config::Config::parse(
            "auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}",
        )?;
        let resolved = fs
            .config
            .resolve_spawn(crate::config::SpawnOptions::default())?;
        let source = Source {
            agent: crate::runtime::test_support::AgentBuilder::new("source").build(),
            instructions: "x".repeat(INSTRUCTION_SNAPSHOT_BYTES + 1),
        };
        let mut args = SpawnArgs::default();
        assert_eq!(
            instructions(&fs, &args, &resolved, Some(&source))
                .unwrap_err()
                .code(),
            "invalid_request"
        );
        args.instructions = String::new().into();
        assert!(instructions(&fs, &args, &resolved, Some(&source))?.is_empty());
        Ok(())
    }
}
