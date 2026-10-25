use super::{
    Context, Exit,
    command::{Command, LocalCommand},
};
use comb::Result;
use hive_setup_support::service::Spec;

pub(super) fn prepare(
    context: &Context,
    path: Option<&std::path::Path>,
    command: &Command,
) -> Result<Option<Spec>> {
    let managed = match path {
        Some(path) => {
            if !matches!(command, Command::Local(LocalCommand::Serve { .. })) {
                return Err(comb::Error::managed_serve_only());
            }
            Some(hive_setup_support::managed::enter(
                path,
                &comb::services::scope(&context.comb)?,
            )?)
        }
        None => None,
    };
    if managed.is_none()
        && matches!(
            command,
            Command::Local(LocalCommand::Setup(_) | LocalCommand::Model(_))
        )
    {
        let layout = hive_setup_support::service::Layout::discover()?;
        let spec =
            hive_setup_support::service::load(&layout, &comb::services::scope(&context.comb)?)?;
        let requested = match command {
            Command::Local(LocalCommand::Setup(input)) => {
                input.options().service_environment.as_deref()
            }
            _ => None,
        }
        .map(hive_setup_support::files::absolute)
        .transpose()?;
        if let Some(spec) = &spec
            && requested.is_some()
            && requested != spec.environment_file
        {
            return Err(comb::Error::service_environment_conflict());
        }
        let environment = requested
            .as_deref()
            .or_else(|| spec.as_ref().and_then(|s| s.environment_file.as_deref()));
        // SAFETY: CLI entry, before any health runtime or provider thread exists.
        unsafe { hive_setup_support::managed::load_values(environment) }?;
    }
    Ok(managed)
}

pub(super) fn execute(
    managed: Option<Spec>,
    raw: Option<hive_log::raw::Owner>,
    standalone: bool,
    run: impl FnOnce() -> Result<Exit>,
) -> Result<Exit> {
    let mut retention = None;
    let result = (|| {
        if let Some(spec) = &managed
            && let Some(suppressed) = environment(spec)?
        {
            return Ok(suppressed);
        }
        retention = raw.map(hive_log::raw::Owner::start).transpose()?;
        if standalone {
            super::logging::run(run)
        } else {
            run()
        }
    })();
    let result = match result {
        Err(error) if (!standalone || managed.is_some()) && report(managed.as_ref(), &error) => {
            Ok(Exit::Suppressed(super::replies::failure(error)))
        }
        result => result,
    };
    if let Some(retention) = retention
        && !retention.finish(RETENTION_SHUTDOWN_GRACE)
    {
        diagnostic(
            "raw_retention_shutdown",
            &std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Raw output retention shutdown unconfirmed",
            ),
        );
    }
    result
}

pub(super) fn prepared(
    managed: Option<&Spec>,
    result: super::input::Result<()>,
) -> super::input::Result<Option<Exit>> {
    match result {
        Err(failure) => {
            if let Some(error) = failure.operational() {
                if report(managed, error) {
                    return Ok(Some(Exit::Suppressed(super::replies::failure(error))));
                }
            } else if let Some(spec) = managed {
                record(spec, failure.code(), false);
            }
            Err(failure)
        }
        Ok(()) => Ok(None),
    }
}

fn environment(spec: &Spec) -> Result<Option<Exit>> {
    // SAFETY: CLI entry remains single-threaded; no diagnostic/runtime worker yet.
    if let Err(cause) = unsafe { hive_setup_support::managed::load_environment(spec) } {
        let permanent = cause.kind() == std::io::ErrorKind::InvalidInput;
        let error = comb::Error::from(cause);
        if record(spec, "service_environment_invalid", permanent) {
            diagnostic("service_environment", &error);
            return Ok(Some(Exit::Suppressed(super::replies::failure(
                super::replies::EnvironmentError {
                    code: "service_environment_invalid",
                    message: error.to_string(),
                },
            ))));
        }
        return Err(error);
    }
    let root = spec
        .scope
        .paths
        .first()
        .ok_or_else(comb::Error::managed_root_missing)?;
    let names = comb::services::required_environment(root)?;
    let missing =
        hive_setup_support::environment::missing(spec.environment_file.as_deref(), &names)?;
    if !missing.is_empty() {
        return Err(comb::Error::service_environment_missing(&missing));
    }
    Ok(None)
}

fn report(spec: Option<&Spec>, error: &comb::Error) -> bool {
    diagnostic("service_execution", error);
    spec.is_some_and(|spec| {
        record(
            spec,
            error.code.as_str(),
            matches!(
                error.code,
                comb::ErrorCode::InvalidEmbeddingConfig
                    | comb::ErrorCode::EmbeddingSpaceChanged
                    | comb::ErrorCode::EmbeddingReplaceRequired
            ),
        )
    })
}
fn record(spec: &Spec, code: &str, permanent: bool) -> bool {
    match hive_setup_support::managed::startup_failed(spec, code, permanent) {
        Ok(suppressed) => suppressed,
        Err(secondary) => {
            diagnostic("service_failure_record", &secondary);
            false // Publication failure must never change the original error or suppress retries.
        }
    }
}
fn diagnostic(phase: &'static str, error: &(dyn std::error::Error + 'static)) {
    if let Err(cause) = super::logging::service_failure(phase, error) {
        // Environment loading has returned. No sink exists; fixed native metadata only.
        eprintln!(
            "Comb: diagnostic setup failed; errno={}.",
            cause.raw_os_error().unwrap_or(0)
        );
    }
}

const RETENTION_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secondary_publication_failure_preserves_preparation_error() {
        use hive_setup_support::service::{Layout, Platform, Scope};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let layout = Layout {
            home: root.clone(),
            config: root.join("config"),
            state: root.join("state"),
            platform: Platform::current(),
        };
        let spec = Spec::new(
            &layout,
            Scope::new("comb", &[&root]).unwrap(),
            std::path::Path::new("/bin/echo"),
            vec!["serve".into()],
            None,
            vec![],
        )
        .unwrap();
        let failure = spec.descriptor.with_extension("failure.json");
        std::fs::create_dir_all(&failure).unwrap();
        let primary = comb::Error::managed_model_changed()
            .caused_by(std::io::Error::from_raw_os_error(libc::EIO));
        let result = prepared(Some(&spec), Err(primary.into()));
        let retained = result.err().unwrap();
        assert_eq!(retained.code(), "invalid_embedding_config");
        assert!(
            retained
                .operational()
                .unwrap()
                .diagnostic()
                .unwrap()
                .contains("Input/output error")
        );
        assert!(failure.is_dir());
    }
}
