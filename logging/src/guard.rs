use crate::{
    Config, Counters, Counts, Fields, Level, Logger,
    fallback::{Fallback, Spawn, Threads},
    producer::Shared,
    record::encode,
    sink::{FileSink, Sink},
};
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shutdown {
    Drained,
    WriteFailed,
    Panicked,
    Unconfirmed,
}
#[derive(Clone, Copy, Debug)]
pub struct Finished {
    pub state: Shutdown,
    pub counts: Counts,
}
/// Owns shutdown; dropping it requests stop but never waits for a blocked sink.
pub struct Guard {
    logger: Logger,
    done: Receiver<Shutdown>,
    writer: JoinHandle<()>,
}
/// Open an owned private JSONL stream. The parent directory must already exist.
/// Startup errors are returned; no failed initialization is cached globally.
#[cfg(test)]
pub fn start(path: &Path, config: Config) -> io::Result<(Logger, Guard)> {
    start_owned(path, config, Box::new(()))
}
pub(crate) fn start_owned(
    path: &Path,
    config: Config,
    owner: Box<dyn Send>,
) -> io::Result<(Logger, Guard)> {
    config.validate()?;
    let sink = FileSink::open_owned(path, &config, owner)?;
    start_with(
        config,
        Box::new(sink),
        Box::new(crate::stderr::descriptor()?),
        &Threads,
    )
}

pub(crate) fn start_stderr(config: Config) -> io::Result<(Logger, Guard)> {
    config.validate()?;
    let sink = crate::stderr::StderrSink(Box::new(crate::stderr::descriptor()?));
    start_with(
        config,
        Box::new(sink),
        Box::new(crate::stderr::descriptor()?),
        &Threads,
    )
}
/// Human diagnostics and fallback notices to `writer`, for a process whose stderr
/// is temporarily unusable, such as while a full-screen terminal view is open.
pub(crate) fn start_writer(
    config: Config,
    writer: impl io::Write + Send + Clone + 'static,
) -> io::Result<(Logger, Guard)> {
    config.validate()?;
    let sink = crate::stderr::StderrSink(Box::new(writer.clone()));
    start_with(config, Box::new(sink), Box::new(writer), &Threads)
}
pub(crate) fn start_with(
    config: Config,
    sink: Box<dyn Sink>,
    stderr: Box<dyn io::Write + Send>,
    spawn: &impl Spawn,
) -> io::Result<(Logger, Guard)> {
    config.validate()?;
    let counters = Arc::new(Counters::default());
    let fallback = Fallback::start(counters.clone(), stderr, spawn)?;
    let (normal_tx, normal_rx) = mpsc::sync_channel(config.queue_slots);
    let (priority_tx, priority_rx) = mpsc::sync_channel(4);
    let (done_tx, done) = mpsc::sync_channel(1);
    let shared = Arc::new(Shared {
        config,
        counters,
        closing: AtomicBool::new(false),
        admitting: AtomicUsize::new(0),
    });
    let worker_state = shared.clone();
    let worker_fallback = fallback.clone();
    let writer = match spawn.spawn(
        "hive-log-writer",
        Box::new(move || {
            let result =
                crate::worker::run(worker_state, normal_rx, priority_rx, sink, worker_fallback);
            let _ = done_tx.try_send(result);
        }),
    ) {
        Ok(writer) => writer,
        Err(error) => {
            fallback.close();
            return Err(error);
        }
    };
    let logger = Logger {
        shared,
        normal: normal_tx,
        priority: priority_tx,
        fallback,
        writer: writer.thread().clone(),
    };
    Ok((
        logger.clone(),
        Guard {
            logger,
            done,
            writer,
        },
    ))
}
impl Guard {
    /// Stop accepting new records and drain admitted records within `timeout`.
    /// Producer clones may remain alive. On timeout the writer keeps its stream
    /// lock until its blocked operation actually returns and shutdown completes.
    pub fn finish(self, timeout: Duration) -> Finished {
        self.close();
        let state = match self.done.recv_timeout(timeout) {
            Ok(state) => state,
            Err(mpsc::RecvTimeoutError::Disconnected) => Shutdown::Panicked,
            Err(mpsc::RecvTimeoutError::Timeout) => Shutdown::Unconfirmed,
        };
        if state != Shutdown::Drained {
            // Best effort on the prestarted bounded worker. Never block finish on stderr.
            if let Ok(bytes) = encode(
                Level::Error,
                "log.finish_failed",
                Fields::default(),
                self.logger.shared.config.record_bytes,
            ) {
                self.logger
                    .fallback
                    .send(bytes, None, &self.logger.shared.counters);
            } else {
                self.logger
                    .shared
                    .counters
                    .invalid
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Finished {
            state,
            counts: self.logger.counts(),
        }
    }
    fn close(&self) {
        self.logger.shared.closing.store(true, Ordering::SeqCst);
        self.writer.thread().unpark();
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.close();
        self.logger.fallback.close();
        // JoinHandle is detached: never join an unresponsive disk or stderr.
    }
}
