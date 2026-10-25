//! Thread-local adapter faults; subprocess death tests install their own hook.
use atomic_fs::Step;
use std::{cell::RefCell, io, path::Path};
type Hook = Box<dyn FnMut(&Path, Step) -> io::Result<()>>;
thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}
pub(crate) fn step(path: &Path, step: Step) -> io::Result<()> {
    HOOK.with(|hook| match hook.borrow_mut().as_mut() {
        Some(hook) => hook(path, step),
        None => Ok(()),
    })
}
pub(crate) struct Guard;
pub(crate) fn install(hook: impl FnMut(&Path, Step) -> io::Result<()> + 'static) -> Guard {
    HOOK.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(Box::new(hook));
    });
    Guard
}
impl Drop for Guard {
    fn drop(&mut self) {
        HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}
