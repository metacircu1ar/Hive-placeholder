//! Thread-local injection at real publication boundaries; no production callbacks.
use hive_setup_support::checkpoints::Checkpoint;
use std::{cell::RefCell, io, path::Path};
type Hook = Box<dyn Fn(&Path, Checkpoint) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
pub(crate) fn with<T>(
    hook: impl Fn(&Path, atomic_fs::Step) -> io::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    with_checkpoint(
        move |path, at| match at {
            Checkpoint::Publication(step) => hook(path, step),
            _ => Ok(()),
        },
        run,
    )
}
pub(crate) fn with_checkpoint<T>(
    hook: impl Fn(&Path, Checkpoint) -> io::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
pub(crate) fn step(path: &Path, at: impl Into<Checkpoint>) -> io::Result<()> {
    HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(path, at.into())))
}
