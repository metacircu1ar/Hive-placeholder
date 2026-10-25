//! Standalone processes own diagnostics through operational cleanup, never daemon files.
use super::{Command, CommandOutcome, DaemonCommand, Error, Result, dispatch, services};
use hive_setup_support::service::Spec;
use std::time::Duration;

const FINISH_BUDGET: Duration = Duration::from_millis(250);

/// Managed startup precedes the daemon logger, and suppression follows it. A
/// thread-scoped sink reports either failure without reopening daemon files
/// or attempting to replace the process-global subscriber.
pub(super) fn service_failure(
    phase: &'static str,
    error: &(dyn std::error::Error + 'static),
) -> std::io::Result<()> {
    let (backend, guard) = hive_log::start_stderr(hive_log::Config::default())?;
    backend.scope(|| {
        tracing::warn!(name: "cli.service_failed",
             phase = phase,
             error = error,
             decision = "preserve_primary_failure");
    });
    guard.finish(FINISH_BUDGET);
    Ok(())
}

pub(super) fn execute(
    prepared: dispatch::Prepared,
    spec: &mut Option<Spec>,
) -> Result<CommandOutcome> {
    let raw = prepare_service(&prepared.cli, spec)?
        .map(hive_log::raw::Owner::start)
        .transpose()
        .map_err(Error::state_io_from)?;
    let result = if matches!(
        prepared.cli.command,
        Command::Daemon {
            command: DaemonCommand::Run
        }
    ) {
        // The daemon installs its subscriber after taking exclusive ownership.
        dispatch::execute(prepared)
    } else {
        run(|| dispatch::execute(prepared))
    };
    if let Some(raw) = raw
        && !raw.finish(FINISH_BUDGET)
    {
        let failure = std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Raw output retention shutdown unconfirmed",
        );
        if let Err(cause) = service_failure("raw_retention_shutdown", &failure) {
            eprintln!(
                "Hive: raw retention shutdown and diagnostic setup unconfirmed; errno={}.",
                cause.raw_os_error().unwrap_or(0)
            );
        }
    }
    result
}

fn prepare_service(
    cli: &super::Cli,
    spec: &mut Option<Spec>,
) -> Result<Option<hive_log::raw::Owner>> {
    let mut prepare = || {
        *spec = services::prepare(cli)?;
        if let Some(spec) = spec.as_ref() {
            services::validate_current(spec)?;
            // SAFETY: no logging, runtime, provider or worker threads exist yet.
            unsafe { hive_setup_support::managed::load_environment(spec) }
                .map_err(services::error)?;
        }
        let raw = if matches!(
            cli.command,
            Command::Daemon {
                command: DaemonCommand::Run
            }
        ) {
            let _configuration = crate::runtime::daemon_configuration(&cli.config_path()?)?;
            let admission = crate::state::State::new(cli.state_root()?)?.admission()?;
            let path = spec.as_ref().map_or_else(
                || cli.state_root().map(|root| root.join("runtime/daemon.log")),
                |spec| Ok(spec.log.clone()),
            )?;
            hive_log::raw::Owner::admit(&path, &std::io::stderr(), admission)
                .map_err(Error::state_io_from)?
        } else {
            None
        };
        Ok(raw)
    };
    prepare().inspect_err(|error| {
        // Environment loading has ended; it is now safe to start a sink worker.
        // The original result is still emitted if no sink can be started.
        if let Err(cause) = service_failure("service_prepare", error) {
            eprintln!(
                "Hive: service diagnostic setup failed; errno={}.",
                cause.raw_os_error().unwrap_or(0)
            );
        }
    })
}

pub(super) fn run<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    run_with(hive_log::start_stderr, work, &mut None)
}

/// A full-screen view owns the terminal: diagnostics wait in a bounded buffer
/// and reach stderr after the view returns, also after a panic.
pub(super) fn run_held<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let held = Held::default();
    let mut finished = None;
    // A panic is caught only until the sink has finished and every held line,
    // the panic record included, is replayed; then it continues.
    let outcome = run_with(
        |config| hive_log::start_writer(config, held.clone()),
        || match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
            Ok(result) => result.map(Ok),
            Err(panic) => Ok(Err(panic)),
        },
        &mut finished,
    );
    held.replay(finished.map(|finished| finished.state));
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(error) => Err(error),
    }
}

fn run_with<T>(
    start: impl FnOnce(hive_log::Config) -> std::io::Result<(hive_log::Backend, hive_log::Guard)>,
    work: impl FnOnce() -> Result<T>,
    finished: &mut Option<hive_log::Finished>,
) -> Result<T> {
    // A broken diagnostic override must not block inspecting/stopping a daemon.
    // Daemon startup independently validates its configuration before mutation.
    let (config, configuration_error) = match crate::runtime::logging_configuration() {
        Ok(config) => (config, None),
        Err(error) => (hive_log::Config::default(), Some(error)),
    };
    let (backend, guard) = start(config).map_err(Error::state_io_from)?;
    if let Err(error) = backend.clone().install_global() {
        *finished = Some(guard.finish(FINISH_BUDGET));
        return Err(Error::state_io_from(std::io::Error::other(error)));
    }
    backend.install_panic_hook();
    if let Some(error) = configuration_error {
        tracing::warn!(name: "cli.log_level_fallback",
             code = error.code(),
             error = &error as &(dyn std::error::Error + 'static),
             decision = "use_default_cli_level");
    }
    // work owns its command values; their destructors run before the sink closes.
    let result = work();
    crate::observability::finish();
    if let Err(error) = &result {
        tracing::error!(name: "cli.failed",
             code = error.code(),
             error = error as &(dyn std::error::Error + 'static),
             decision = "preserve_result");
    }
    // Logger failure already schedules a bounded fallback notice. Never replace
    // an accepted command result and invite a retry of an uncertain mutation.
    *finished = Some(guard.finish(FINISH_BUDGET));
    result
}

// Diagnostics held while a full-screen view is open; later lines are counted.
const HELD_BYTES: usize = 64 * 1024;

#[derive(Clone, Default)]
struct Held(std::sync::Arc<std::sync::Mutex<(Vec<u8>, u64)>>);
impl std::io::Write for Held {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // Appending is panic-free, so a poisoned lock still holds whole lines.
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.0.len() + bytes.len() <= HELD_BYTES {
            held.0.extend_from_slice(bytes);
        } else {
            held.1 += 1;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Held {
    /// Write every held line to stderr, then what was dropped or left unconfirmed.
    fn replay(&self, state: Option<hive_log::Shutdown>) {
        use std::io::Write;
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (bytes, dropped) = std::mem::take(&mut *held);
        let mut stderr = std::io::stderr().lock();
        // Best effort: stderr is the only remaining place to report them.
        let _ = stderr.write_all(&bytes);
        if dropped > 0 {
            let _ = writeln!(
                stderr,
                "Hive: {dropped} diagnostic lines were dropped while the view was open."
            );
        }
        if state.is_some_and(|state| state != hive_log::Shutdown::Drained) {
            let _ = writeln!(
                stderr,
                "Hive: diagnostics may be incomplete; the logger did not confirm its shutdown."
            );
        }
    }
}
