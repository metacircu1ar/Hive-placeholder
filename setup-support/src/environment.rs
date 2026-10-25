//! Explicit service credentials, never a snapshot of the invoking shell.
use crate::{Result, files, invalid};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const PROXIES: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

pub fn read(path: Option<&Path>) -> Result<BTreeMap<String, String>> {
    let Some(path) = path else {
        return Ok(BTreeMap::new());
    };
    let bytes =
        files::read(path)?.ok_or_else(|| invalid("Service environment file is missing."))?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| invalid("Service environment must be UTF-8."))?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("Service environment requires KEY=literal-value lines."))?;
        if !valid_key(key)
            || matches!(
                key,
                "HOME"
                    | "PATH"
                    | "XDG_CONFIG_HOME"
                    | "XDG_STATE_HOME"
                    | "XDG_DATA_HOME"
                    | "XDG_RUNTIME_DIR"
            )
            || value.contains('\0')
            || values.insert(key.into(), value.into()).is_some()
        {
            return Err(invalid(
                "Invalid or duplicate service environment key, or a NUL value.",
            ));
        }
    }
    Ok(values)
}
pub fn valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
pub fn required(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .chain(
            PROXIES
                .iter()
                .filter(|n| std::env::var_os(n).is_some())
                .map(|n| (*n).to_owned()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub fn missing(path: Option<&Path>, names: &[String]) -> Result<Vec<String>> {
    let values = read(path)?;
    Ok(names
        .iter()
        .filter(|key| values.get(*key).is_none_or(String::is_empty))
        .cloned()
        .collect())
}

/// Capture a reproducible toolchain search path without cwd-dependent entries.
pub fn service_path(value: Option<&str>) -> crate::Result<String> {
    let input = value.map_or_else(
        || std::env::var_os("PATH").unwrap_or_default(),
        std::ffi::OsString::from,
    );
    let mut entries = Vec::new();
    for path in std::env::split_paths(&input) {
        if value.is_some()
            && (!path.is_absolute()
                || !path.is_dir()
                || path
                    .to_str()
                    .is_none_or(|s| s.chars().any(char::is_control)))
        {
            return Err(crate::invalid(
                "Explicit service PATH requires absolute existing directories without control \
                    characters.",
            ));
        }
        if path.is_absolute() && path.is_dir() && !entries.contains(&path) {
            entries.push(path);
        }
    }
    if entries.is_empty() {
        return Err(crate::invalid(
            "Service PATH has no absolute existing directories; pass --service-path.",
        ));
    }
    std::env::join_paths(entries)
        .map_err(|_| crate::invalid("Invalid service PATH."))?
        .into_string()
        .map_err(|_| crate::invalid("Service PATH must be UTF-8."))
}

/// Warn about a script interpreter that will be unavailable to the login service.
pub fn interpreter_warning(program: &std::path::Path, path: &str) -> crate::Result<Option<String>> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(program)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let Some(line) = text.lines().next().and_then(|s| s.strip_prefix("#!")) else {
        return Ok(None);
    };
    let mut words = line.split_whitespace();
    if words.next() != Some("/usr/bin/env") {
        return Ok(None);
    }
    let mut interpreter = words.next();
    if interpreter == Some("-S") {
        interpreter = words.next();
    }
    let Some(interpreter) = interpreter else {
        return Ok(None);
    };
    if interpreter.starts_with('-') || interpreter.contains('=') {
        return Ok(Some(format!(
            "Inspect {}: its env shebang cannot be checked against the recorded service PATH.",
            program.display()
        )));
    }
    let mut found = false;
    for directory in std::env::split_paths(path) {
        match std::fs::metadata(directory.join(interpreter)) {
            Ok(meta) if meta.is_file() && meta.mode() & 0o111 != 0 => {
                found = true;
                break;
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(crate::context("Cannot inspect service interpreter", error)),
        }
    }
    Ok((!found).then(|| {
        format!(
            "{} needs {interpreter}, which is missing from the recorded service PATH. \
        Reinstall the service with --service-path.",
            program.display()
        )
    }))
}
