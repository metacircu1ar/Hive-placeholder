//! Keep a background owner's context alive through completion notification and panic
//! propagation.
pub(super) fn spawn_owned_thread<C, T>(
    context: C,
    work: impl FnOnce(&C) -> T + Send + 'static,
    completed: impl FnOnce(&C, &std::thread::Result<T>) + Send + 'static,
) -> std::thread::JoinHandle<T>
where
    C: Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&context)));
        completed(&context, &result);
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Thread ownership fixtures deliberately panic on unexpected results.
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct Owner(Arc<AtomicBool>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn owned_thread_notifies_before_drop_and_preserves_error_or_panic() {
        for panic in [false, true] {
            let dropped = Arc::new(AtomicBool::new(false));
            let completed = Arc::new(AtomicUsize::new(0));
            let observed = completed.clone();
            let thread = spawn_owned_thread(
                Owner(dropped.clone()),
                move |_| {
                    if panic {
                        std::panic::panic_any(17_u32);
                    }
                    Err::<(), _>("work failed")
                },
                move |owner, result| {
                    assert!(!owner.0.load(Ordering::SeqCst));
                    assert_eq!(result.is_err(), panic);
                    observed.fetch_add(1, Ordering::SeqCst);
                },
            );
            let result = thread.join();
            if panic {
                assert_eq!(*result.unwrap_err().downcast::<u32>().unwrap(), 17);
            } else {
                assert_eq!(result.unwrap(), Err("work failed"));
            }
            assert!(dropped.load(Ordering::SeqCst));
            assert_eq!(completed.load(Ordering::SeqCst), 1);
        }
    }
}
