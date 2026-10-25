use super::{Path, PathBuf, Result, invalid};
use std::path::Component;
pub(super) fn home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| invalid("Set an absolute HOME or pass --root."))?;
    Ok(home)
}
pub(super) fn bridge() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .map_or_else(|| home().map(|h| h.join(".config")), Ok)?;
    if !base.is_absolute() {
        return Err(invalid(
            "XDG_CONFIG_HOME must be absolute; or pass --bridge-file.",
        ));
    }
    absolute(&base.join("hive/comb-bridge.json"))
}
pub(super) fn absolute(path: &Path) -> Result<PathBuf> {
    if path.components().any(|p| matches!(p, Component::ParentDir)) {
        return Err(invalid("Setup paths must not contain '..'."));
    }
    let path = std::path::absolute(path)?;
    if hive_setup_support::files::optional_metadata(&path)?
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(invalid("Setup refuses a symbolic link at a managed path."));
    }
    // A managed leaf may not be a link. Ancestors may be: /tmp on macOS,
    // for example, resolves to /private/tmp even before the leaf exists.
    let mut ancestor = path.as_path();
    let mut missing = Vec::new();
    while !ancestor.try_exists()? {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| invalid("Invalid setup filename."))?,
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| invalid("Invalid setup path."))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}
