//! Stable installed paths and bounded structured child commands.
use super::{Result, invalid, io};
mod response;
use hive_setup_support::files;
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Deserialize)]
pub(super) struct Identity {
    pub product: String,
    pub compiled_features: Vec<String>,
}
pub(super) fn call<T: DeserializeOwned>(
    binary: &Path,
    args: &[String],
    timeout: Duration,
    inputs: &[&str],
) -> Result<T> {
    let path = binary
        .to_str()
        .ok_or_else(|| invalid("Executable path must be UTF-8."))?;
    let output =
        hive_setup_support::runner::capture(crate::os::spawn, path, args, timeout).map_err(io)?;
    response::decode(inputs, output)
}
pub(super) fn progress<T: DeserializeOwned>(
    binary: &Path,
    args: &[String],
    inputs: &[&str],
) -> Result<T> {
    let cancellation = crate::process::Cancellation::install()?;
    let path = binary
        .to_str()
        .ok_or_else(|| invalid("Executable path must be UTF-8."))?;
    let output = hive_setup_support::runner::progress(crate::os::spawn, path, args, &|| {
        cancellation.count() != 0
    })
    .map_err(io)?;
    response::decode(inputs, output)
}
pub(super) fn invoked() -> Result<PathBuf> {
    let arg = std::env::args_os()
        .next()
        .ok_or_else(|| invalid("Cannot locate invoked executable."))?;
    files::executable(Path::new(&arg)).map_err(io)
}
pub(in crate::cli) fn skills(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return files::location(path).map_err(io);
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    if crate::executable::path()?.starts_with(source.join("target/release")) {
        return Ok(source.join("skills"));
    }
    let binary = invoked()?;
    let prefix = binary
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("Cannot locate installed skills; pass --skills-dir."))?;
    Ok(prefix.join("share/hive/skills"))
}
pub(super) fn comb(explicit: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        return files::executable(path).map(Some).map_err(io);
    }
    let sibling = invoked()?.parent().map(|p| p.join("comb"));
    if let Some(path) = sibling
        && path.is_file()
    {
        return files::executable(&path).map(Some).map_err(io);
    }
    Ok(crate::observability::optional(
        files::executable(Path::new("comb")),
        "setup_optional_comb",
    ))
}
