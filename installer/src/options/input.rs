//! Read-only caller path preparation before staging or process execution.
use super::{Args, parsing::EXAMPLES};
use crate::Result;
use hive_input::{Code, InputError};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

fn invalid(
    code: Code,
    field: &str,
    message: &str,
    expected: &str,
) -> Box<dyn std::error::Error + Send + Sync> {
    match InputError::new(
        code,
        message,
        field,
        expected,
        EXAMPLES.iter().map(|s| (*s).into()).collect(),
    ) {
        Ok(error) => Box::new(error),
        Err(error) => Box::new(error),
    }
}
pub(super) fn prepare(args: &mut Args) -> Result<()> {
    for (field, path) in [
        ("--from", args.source.as_deref()),
        ("--staged-from", args.staged_from.as_deref()),
        ("--config", args.config.as_deref()),
        ("--state-dir", args.state_dir.as_deref()),
        ("--comb-root", args.comb_root.as_deref()),
        ("--bridge-file", args.bridge_file.as_deref()),
        ("--purge-comb-root", args.purge_comb_root.as_deref()),
    ] {
        if path.is_some_and(|path| path.as_os_str().is_empty()) {
            return Err(invalid(
                Code::InvalidPath,
                field,
                "The path is empty.",
                "a nonempty path",
            ));
        }
    }
    args.prefix = Some(prefix(args.prefix.as_deref())?);
    let source = args
        .staged_from
        .as_ref()
        .map(|p| ("--staged-from", p))
        .or_else(|| args.source.as_ref().map(|p| ("--from", p)));
    if let Some((field, path)) = source {
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => (),
            Ok(_) => {
                return Err(invalid(
                    Code::InvalidPath,
                    field,
                    "Release source is not a directory.",
                    "an existing directory containing release assets",
                ));
            }
            Err(error) => return Err(path_error(field, error)),
        }
    }
    Ok(())
}
pub(super) fn prefix(selected: Option<&Path>) -> Result<PathBuf> {
    let path = selected
        .map(Path::to_owned)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".local"))
        })
        .ok_or_else(|| {
            invalid(
                Code::MissingArgument,
                "--prefix",
                "HOME is unavailable.",
                "pass an absolute --prefix directory or set HOME",
            )
        })?;
    validate_prefix(&path)?;
    hive_setup_support::files::absolute(&path).map_err(|error| path_error("--prefix", error))
}
pub(crate) fn validate_prefix(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(invalid(
            Code::InvalidPath,
            "--prefix",
            "Invalid installation prefix.",
            "a nonempty absolute directory without parent traversal",
        ));
    }
    let normalized: PathBuf = path.components().collect();
    for (index, ancestor) in normalized.ancestors().enumerate() {
        let metadata = if index == 0 {
            fs::symlink_metadata(ancestor)
        } else {
            fs::metadata(ancestor)
        };
        match metadata {
            Ok(meta) if meta.is_dir() => return Ok(()),
            Ok(_) => {
                return Err(invalid(
                    Code::InvalidPath,
                    "--prefix",
                    "Installation prefix is not a directory or is a leaf symlink.",
                    "a directory with an existing directory ancestor; no leaf symlink",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if hive_setup_support::files::optional_metadata(ancestor)
                    .map_err(|cause| path_error("--prefix", cause))?
                    .is_some_and(|meta| meta.file_type().is_symlink())
                {
                    return Err(invalid(
                        Code::InvalidPath,
                        "--prefix",
                        "Prefix contains a dangling symlink.",
                        "an existing directory ancestor",
                    ));
                }
            }
            Err(error) => return Err(path_error("--prefix", error)),
        }
    }
    Err(invalid(
        Code::InvalidPath,
        "--prefix",
        "Installation prefix has no directory ancestor.",
        "an existing directory ancestor",
    ))
}
fn path_error(field: &str, error: std::io::Error) -> Box<dyn std::error::Error + Send + Sync> {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::InvalidInput
    ) {
        invalid(
            Code::InvalidPath,
            field,
            "Cannot access the selected path.",
            "an accessible directory with valid parent components",
        )
    } else {
        Box::new(error)
    }
}
