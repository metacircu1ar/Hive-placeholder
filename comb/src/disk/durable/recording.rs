//! Per-owner durable I/O observations and deterministic one-shot failures.
use super::{Durability, System};
use crate::{Error, ErrorCode, Result, error::CommitExt};
use std::{fs::File, io, sync::Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Fsync,
    OrderingBarrier,
    FullFlush,
    DirectorySync,
    BeforeFinish,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Counts {
    pub full_barriers: usize,
    pub order_fallbacks: usize,
    pub events: Vec<Operation>,
}

struct Failure {
    operation: Operation,
    remaining: usize,
}

#[derive(Default)]
struct State {
    counts: Counts,
    failure: Option<Failure>,
    finish_failure: bool,
    // None uses the real OS; Some controls only the optional barrier primitive.
    barrier: Option<std::result::Result<(), i32>>,
}

#[derive(Default)]
pub(crate) struct Recording {
    state: Mutex<State>,
}

impl Recording {
    pub fn reset(&self) {
        self.state.lock().unwrap().counts = Counts::default();
    }
    pub fn counts(&self) -> Counts {
        self.state.lock().unwrap().counts.clone()
    }
    pub fn fail_finish_once(&self) {
        self.state.lock().unwrap().finish_failure = true;
    }
    pub fn fail(&self, operation: Operation, occurrence: usize) {
        assert!(occurrence > 0);
        self.state.lock().unwrap().failure = Some(Failure {
            operation,
            remaining: occurrence,
        });
    }
    pub fn barrier(&self, result: std::result::Result<(), i32>) {
        self.state.lock().unwrap().barrier = Some(result);
    }
    fn enter(&self, operation: Operation) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.counts.events.push(operation);
        if operation == Operation::FullFlush {
            state.counts.full_barriers += 1;
        }
        if let Some(failure) = &mut state.failure
            && failure.operation == operation
        {
            failure.remaining -= 1;
            if failure.remaining == 0 {
                state.failure = None;
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
        }
        Ok(())
    }
}

impl Durability for Recording {
    fn fsync(&self, file: &File) -> io::Result<()> {
        self.enter(Operation::Fsync)?;
        System.fsync(file)
    }
    fn ordering_barrier(&self, file: &File) -> io::Result<()> {
        self.enter(Operation::OrderingBarrier)?;
        let forced = self.state.lock().unwrap().barrier;
        let result = match forced {
            Some(result) => result.map_err(io::Error::from_raw_os_error),
            None => System.ordering_barrier(file),
        };
        if result.as_ref().err().is_some_and(|error| {
            matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS)
            )
        }) {
            self.state.lock().unwrap().counts.order_fallbacks += 1;
        }
        result
    }
    fn full_flush(&self, file: &File) -> io::Result<()> {
        self.enter(Operation::FullFlush)?;
        System.full_flush(file)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        self.enter(Operation::DirectorySync)?;
        System.sync_directory(file)
    }
    fn before_finish(&self) -> Result<()> {
        self.enter(Operation::BeforeFinish).committed()?;
        let mut state = self.state.lock().unwrap();
        if std::mem::take(&mut state.finish_failure) {
            return Err(
                Error::new(ErrorCode::IoError, "Injected finalization failure.").committed(),
            );
        }
        Ok(())
    }
}
