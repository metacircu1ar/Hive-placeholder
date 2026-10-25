//! Managed execution metadata. Health checks, never this record, decide readiness.
use crate::{
    Result, files, invalid,
    service::{Layout, Scope, Spec},
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
static READY: AtomicBool = AtomicBool::new(false);
static LABEL: OnceLock<String> = OnceLock::new();
pub fn label() -> Option<&'static str> {
    LABEL.get().map(String::as_str)
}
pub fn ready() {
    READY.store(true, Ordering::Release);
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub code: String,
    pub suppressed: bool,
}
pub fn failure(spec: &Spec) -> Result<Option<Failure>> {
    let failure: Option<Failure> = files::json(&spec.descriptor.with_extension("failure.json"))?;
    if let Some(failure) = &failure {
        valid_code(&failure.code)?;
    }
    Ok(failure)
}
pub fn clear_failure(spec: &Spec) -> Result<()> {
    // Reject unsupported development records instead of treating them as current.
    failure(spec)?;
    let path = spec.descriptor.with_extension("failure.json");
    if let Some(bytes) = files::read(&path)? {
        files::remove(&path, &bytes)?;
    }
    Ok(())
}
pub fn enter(path: &Path, scope: &Scope) -> Result<Spec> {
    let layout = Layout::discover()?;
    if path != layout.descriptor(scope)? {
        return Err(invalid(
            "Managed execution descriptor is for another scope.",
        ));
    }
    let spec = super::service::load(&layout, scope)?
        .ok_or_else(|| invalid("Managed service is not installed."))?;
    clear_failure(&spec)?;
    LABEL
        .set(scope.label()?)
        .map_err(|_| invalid("Managed execution is already initialized."))?;
    Ok(spec)
}
/// Must be called at the single-threaded CLI entry, before any runtime, engine,
/// credential recovery or provider process is created.
///
/// # Safety
/// No other thread may read or modify the process environment while this runs.
pub unsafe fn load_environment(spec: &Spec) -> Result<()> {
    spec.check_environment()?;
    // SAFETY: same single-threaded caller contract.
    unsafe { load_values(spec.environment_file.as_deref()) }
}
/// Read the explicit file for an operator's model validation, without inheriting
/// the service's possibly obsolete required-name set.
///
/// # Safety
/// The CLI must still be single-threaded; no concurrent environment access.
pub unsafe fn load_values(path: Option<&Path>) -> Result<()> {
    for (key, value) in crate::environment::read(path)? {
        // SAFETY: the CLI caller guarantees single-threaded execution.
        unsafe {
            std::env::set_var(key, value);
        }
    }
    Ok(())
}
pub fn startup_failed(spec: &Spec, code: &str, permanent: bool) -> Result<bool> {
    valid_code(code)?;
    if READY.load(Ordering::Acquire) {
        return Ok(false);
    }
    files::write(
        &spec.descriptor.with_extension("failure.json"),
        &files::encode(&Failure {
            code: code.into(),
            suppressed: permanent,
        })?,
        true,
    )?;
    Ok(permanent)
}

fn valid_code(code: &str) -> Result<()> {
    if code.is_empty()
        || code.len() > 128
        || !code
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(invalid("Invalid managed startup failure code."));
    }
    Ok(())
}
