use crate::{Fields, Invalid, Logger};
use std::panic::PanicHookInfo;

/// Add sanitized diagnostics before the existing hook. The previous hook remains
/// responsible for its own output/privacy and may take longer than our 250 ms.
/// Install once at process startup, not from a panicking thread.
pub fn install_panic_hook(logger: Logger) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = logger.fatal("process.panic", |fields| panic_fields(info, fields));
        previous(info);
    }));
}
fn panic_fields(info: &PanicHookInfo<'_>, fields: &mut Fields) -> Result<(), Invalid> {
    fields.code(
        "thread",
        if crate::producer::LOG_WORKER.get() {
            "log_writer"
        } else {
            "application"
        },
    )?;
    fields.code(
        "payload",
        if info.payload().is::<&str>() || info.payload().is::<String>() {
            "string"
        } else {
            "other"
        },
    )?;
    if let Some(location) = info.location() {
        // No filesystem path or caller-controlled thread name is logged.
        fields.location(location.file(), location.line(), location.column())?;
    }
    Ok(())
}
