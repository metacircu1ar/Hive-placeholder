//! A tracing Layer controls capture; only the private producer owns the queue.
mod install;
mod visit;
use crate::{
    Config, Counts, Guard, Logger,
    policy::{self, Policy},
    record::{Fields, Invalid, Level, SpanContext},
};
pub use install::InstallError;
use std::{io, path::Path, sync::Arc};
use tracing::{Event, Metadata, Subscriber, level_filters::LevelFilter, subscriber::Interest};
use tracing_log::NormalizeEvent;
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// Cloneable tracing backend and diagnostic control. Emit via tracing macros.
#[derive(Clone)]
pub struct Backend {
    logger: Logger,
    policy: Arc<Policy>,
}
pub fn start(path: &Path, config: Config) -> io::Result<(Backend, Guard)> {
    start_owned(path, config, ())
}
/// Standalone human diagnostics, with no file or state-directory creation.
/// Shutdown is bounded even when the inherited stderr descriptor is blocked.
pub fn start_stderr(config: Config) -> io::Result<(Backend, Guard)> {
    let policy = Arc::new(Policy::load()?);
    let (logger, guard) = crate::guard::start_stderr(config)?;
    Ok((Backend { logger, policy }, guard))
}
/// Human diagnostics to `writer` instead of stderr; the caller replays them once
/// stderr is usable again.
pub fn start_writer(
    config: Config,
    writer: impl io::Write + Send + Clone + 'static,
) -> io::Result<(Backend, Guard)> {
    let policy = Arc::new(Policy::load()?);
    let (logger, guard) = crate::guard::start_writer(config, writer)?;
    Ok((Backend { logger, policy }, guard))
}
/// Keep an ownership lease in the writer until its sink actually closes, including
/// after a bounded finish times out. Global subscriber clones never hold this lease.
pub fn start_owned(
    path: &Path,
    config: Config,
    owner: impl Send + 'static,
) -> io::Result<(Backend, Guard)> {
    let policy = Arc::new(Policy::load()?);
    let (logger, guard) = crate::guard::start_owned(path, config, Box::new(owner))?;
    Ok((Backend { logger, policy }, guard))
}
impl Backend {
    pub fn counts(&self) -> Counts {
        self.logger.counts()
    }
    pub fn install_panic_hook(&self) {
        crate::panic::install_panic_hook(self.logger.clone());
    }
    fn capture<S>(&self, event: &Event<'_>, context: Context<'_, S>)
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let meta = event.metadata();
        let is_bridge = event.is_log();
        let trusted = policy::first_party(meta);
        let mut visitor =
            visit::Visitor::new(&self.policy, &self.logger.shared.config, trusted, is_bridge);
        event.record(&mut visitor);
        if is_bridge && *meta.level() > visitor.bridge_level {
            return;
        }
        let name = if trusted {
            meta.name()
        } else {
            "library.event"
        };
        let fields = Self::metadata(event, context, visitor);
        if policy::fatal(meta) {
            self.logger.fatal(name, |out| {
                *out = fields?;
                Ok(())
            });
        } else {
            self.logger.admit(Level::from(*meta.level()), name, |out| {
                *out = fields?;
                Ok(())
            });
        }
    }
    fn metadata<S>(
        event: &Event<'_>,
        context: Context<'_, S>,
        visitor: visit::Visitor<'_>,
    ) -> Result<Fields, Invalid>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let meta = event.metadata();
        let mut fields = visitor.finish()?;
        // Never normalize log bridge metadata: target/module/file can be secrets.
        fields.provenance("target", if event.is_log() { "log" } else { meta.target() })?;
        if !event.is_log()
            && let Some(file) = meta.file()
        {
            fields.location(file, meta.line().unwrap_or(0), 0)?;
        }
        if let Some(mut scope) = context.event_scope(event) {
            let mut spans = Vec::with_capacity(8);
            for span in scope.by_ref().take(8) {
                let name = span.metadata().name();
                spans.push(SpanContext {
                    name: if policy::target_name(name) {
                        name
                    } else {
                        "redacted"
                    },
                    id: span.id().into_u64(),
                });
            }
            let truncated = scope.next().is_some();
            spans.reverse();
            fields.context(spans, truncated)?;
        }
        Ok(fields)
    }
}
impl<S> Layer<S> for Backend
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.logger.shared.config.accepts(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }
    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        self.logger.shared.config.accepts(metadata)
    }
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(
            self.logger
                .shared
                .config
                .level
                .max(self.logger.shared.config.library_max())
                .max(LevelFilter::ERROR),
        )
    }
    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        // Another Layer can enable a callsite this sink would otherwise reject.
        if self.logger.shared.config.accepts(event.metadata()) {
            self.capture(event, context);
        }
    }
}

#[cfg(test)]
pub(crate) fn from_logger(logger: Logger) -> Backend {
    Backend {
        logger,
        policy: Arc::new(Policy::load().unwrap()),
    }
}
