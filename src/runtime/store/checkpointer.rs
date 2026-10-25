//! A second connection that copies WAL frames into the database file on its own
//! thread, so that a slow checkpoint sync does not hold the writer.
//!
//! It runs PASSIVE only. TRUNCATE, RESTART and FULL take the WAL write lock, and
//! the writer treats `SQLITE_BUSY` on begin as fatal. The writer's own
//! checkpoints (the idle one, and the fallback when the WAL grows too large) hold
//! the pass lock, so the two connections never checkpoint at the same time.
use super::{Result, files, sql::RuntimeResult as _};
use crate::runtime::locks::MutexExt as _;
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Passes per request: each copies what the writer appended during the previous one.
const PASSES: usize = 8;

pub(in crate::runtime) struct Checkpointer {
    requests: Option<SyncSender<()>>,
    /// The latest request's sequence; a pass covers every request made before it began.
    requested: Arc<AtomicU64>,
    /// The request sequence each complete pass covered.
    completed: Receiver<u64>,
    pass: Arc<Mutex<()>>,
    thread: Option<JoinHandle<()>>,
}
impl Checkpointer {
    pub fn start(path: &Path) -> Result<Self> {
        let connection = files::checkpoint_connection(path)?;
        // One pending request is enough: a pass copies every committed frame.
        let (requests, received) = mpsc::sync_channel(1);
        // Room for a stale completion that lands while the writer drains.
        let (complete, completed) = mpsc::sync_channel(4);
        let requested = Arc::new(AtomicU64::new(0));
        let pass = Arc::new(Mutex::new(()));
        let worker = Worker {
            requested: requested.clone(),
            complete,
            pass: pass.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("hive-checkpoint".into())
            .spawn(move || worker.run(&connection, &received))
            .map_err(crate::error::Error::state_io_from)?;
        Ok(Self {
            requests: Some(requests),
            requested,
            completed,
            pass,
            thread: Some(thread),
        })
    }
    /// Ask for a pass without waiting; requests made during a pass coalesce.
    pub fn request(&self) -> u64 {
        let sequence = self.requested.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some(requests) = &self.requests
            && let Err(TrySendError::Disconnected(())) = requests.try_send(())
        {
            tracing::warn!(name: "store.checkpointer_stopped", decision = "checkpoint_on_writer");
        }
        sequence
    }
    /// Wait for a complete pass that began after this call. The writer appends
    /// nothing meanwhile, so its next write restarts the WAL. It gives up after
    /// `timeout`, so a stalled sync stays off the writer.
    pub fn drain(&self, timeout: Duration) -> bool {
        while self.completed.try_recv().is_ok() {}
        let wanted = self.request();
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .completed
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(covered) if covered >= wanted => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }
    /// Wait for the current pass, then keep the checkpointer out while held.
    pub fn exclusive(&self) -> MutexGuard<'_, ()> {
        self.pass.lock_or_panic()
    }
    /// Finish the current pass and close the connection, so that the writer's
    /// connection closes last and keeps SQLite's close-time checkpoint.
    pub fn stop(mut self) {
        self.halt();
    }
    fn halt(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!(name: "store.checkpointer_panicked", decision = "continue_shutdown");
        }
    }
}
impl Drop for Checkpointer {
    fn drop(&mut self) {
        self.halt();
    }
}
struct Worker {
    requested: Arc<AtomicU64>,
    complete: SyncSender<u64>,
    pass: Arc<Mutex<()>>,
}
impl Worker {
    fn run(&self, connection: &Connection, requests: &Receiver<()>) {
        // A disconnected channel means the writer is stopping.
        while requests.recv().is_ok() {
            // Every request made before this point is covered by a complete pass.
            let covered = self.requested.load(Ordering::Acquire);
            for _ in 0..PASSES {
                let outcome = {
                    let _pass = self.pass.lock_or_panic();
                    passive(connection)
                };
                match outcome {
                    Ok(Pass::Complete) => {
                        // A full channel holds older completions nobody awaits.
                        let _ = self.complete.try_send(covered);
                        break;
                    }
                    Ok(Pass::Busy) => break,
                    Ok(Pass::Partial) => {}
                    Err(error) => {
                        tracing::warn!(name: "store.checkpointer_failed",
                             code = error.code(),
                             error = &error as &(dyn std::error::Error + 'static),
                             sqlite_code = super::sqlite_code(&error),
                             decision = "retry_on_request");
                        break;
                    }
                }
            }
        }
    }
}
enum Pass {
    /// Every committed frame is in the database file.
    Complete,
    /// The writer appended during the pass, or a reader pinned older frames.
    Partial,
    /// Another checkpoint holds SQLite's checkpoint lock.
    Busy,
}
fn passive(connection: &Connection) -> Result<Pass> {
    let (busy, log, copied): (i64, i64, i64) = connection
        .query_row(super::queries::CHECKPOINT_PASSIVE, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .runtime()?;
    Ok(if busy != 0 {
        tracing::debug!(name: "store.checkpoint_deferred",
             phase = "background",
             decision = "defer");
        Pass::Busy
    } else if log == copied {
        Pass::Complete
    } else {
        Pass::Partial
    })
}
