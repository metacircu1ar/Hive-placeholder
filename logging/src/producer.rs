use crate::{Config, Counters, Counts, Fields, Invalid, Level, fallback::Fallback, record::encode};
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

thread_local! { pub(crate) static LOG_WORKER: Cell<bool> = const { Cell::new(false) }; }
pub(crate) const FATAL_BUDGET: Duration = Duration::from_millis(250);

pub(crate) struct Packet {
    pub bytes: Vec<u8>,
    pub receipt: Option<SyncSender<bool>>,
}
pub(crate) struct Shared {
    pub config: Config,
    pub counters: Arc<Counters>,
    pub closing: AtomicBool,
    pub admitting: AtomicUsize,
}
/// Cloneable nonblocking producer. Dropping a clone does not stop the writer.
#[derive(Clone)]
pub struct Logger {
    pub(crate) shared: Arc<Shared>,
    pub(crate) normal: SyncSender<Packet>,
    pub(crate) priority: SyncSender<Packet>,
    pub(crate) fallback: Fallback,
    pub(crate) writer: thread::Thread,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    #[cfg(test)]
    Disabled,
    Queued,
    Full,
    Closed,
    Invalid,
    Oversized,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Receipt {
    DiskConfirmed,
    StderrConfirmed,
    Unconfirmed,
}
struct Admitting<'a>(&'a Shared, &'a thread::Thread);
impl Drop for Admitting<'_> {
    fn drop(&mut self) {
        if self.0.admitting.fetch_sub(1, Ordering::SeqCst) == 1
            && self.0.closing.load(Ordering::SeqCst)
        {
            // Closing may have observed this reservation before its enqueue.
            self.1.unpark();
        }
    }
}
impl Logger {
    #[cfg(test)]
    pub fn enabled(&self, level: Level) -> bool {
        level.tracing() <= self.shared.config.level
    }
    pub fn counts(&self) -> Counts {
        self.shared.counters.snapshot()
    }
    /// Build metadata lazily; disabled records never invoke the closure.
    #[cfg(test)]
    pub fn record(
        &self,
        level: Level,
        event: &'static str,
        fields: impl FnOnce(&mut Fields) -> Result<(), Invalid>,
    ) -> Admission {
        if !self.enabled(level) {
            return Admission::Disabled;
        }
        self.admit(level, event, fields)
    }
    pub(crate) fn admit(
        &self,
        level: Level,
        event: &'static str,
        fields: impl FnOnce(&mut Fields) -> Result<(), Invalid>,
    ) -> Admission {
        let bytes = match self.prepare(level, event, fields) {
            Ok(b) => b,
            Err(a) => return a,
        };
        self.enqueue(
            Packet {
                bytes,
                receipt: None,
            },
            false,
        )
    }
    fn prepare(
        &self,
        level: Level,
        event: &'static str,
        build: impl FnOnce(&mut Fields) -> Result<(), Invalid>,
    ) -> Result<Vec<u8>, Admission> {
        let mut fields = Fields::default();
        let result = build(&mut fields)
            .and_then(|()| encode(level, event, fields, self.shared.config.record_bytes));
        result.map_err(|error| {
            if error == Invalid::Oversized {
                self.shared
                    .counters
                    .oversized
                    .fetch_add(1, Ordering::Relaxed);
                Admission::Oversized
            } else {
                if error == Invalid::NumericRange {
                    self.shared
                        .counters
                        .numeric_invalid
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.shared.counters.invalid.fetch_add(1, Ordering::Relaxed);
                Admission::Invalid
            }
        })
    }
    fn enqueue(&self, packet: Packet, priority: bool) -> Admission {
        // Reservations close the finish-vs-send race without waiting for a consumer.
        self.shared.admitting.fetch_add(1, Ordering::SeqCst);
        let _reservation = Admitting(&self.shared, &self.writer);
        if self.shared.closing.load(Ordering::SeqCst) {
            self.shared
                .counters
                .disconnected
                .fetch_add(1, Ordering::Relaxed);
            return Admission::Closed;
        }
        let sender = if priority {
            &self.priority
        } else {
            &self.normal
        };
        match sender.try_send(packet) {
            Ok(()) => {
                self.writer.unpark();
                Admission::Queued
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.shared.counters.full.fetch_add(1, Ordering::Relaxed);
                Admission::Full
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.shared
                    .counters
                    .disconnected
                    .fetch_add(1, Ordering::Relaxed);
                Admission::Closed
            }
        }
    }
    /// Bypasses normal capacity and filtering. Bounds acknowledgement waiting to
    /// 250 ms; the caller's metadata closure and any prior panic hook are separate.
    pub fn fatal(
        &self,
        event: &'static str,
        fields: impl FnOnce(&mut Fields) -> Result<(), Invalid>,
    ) -> Receipt {
        let receipt = self.deliver_fatal(event, fields);
        if receipt == Receipt::Unconfirmed {
            self.shared
                .counters
                .fatal_unconfirmed
                .fetch_add(1, Ordering::Relaxed);
        }
        receipt
    }
    fn deliver_fatal(
        &self,
        event: &'static str,
        fields: impl FnOnce(&mut Fields) -> Result<(), Invalid>,
    ) -> Receipt {
        let started = Instant::now();
        let Ok(bytes) = self.prepare(Level::Error, event, fields) else {
            return Receipt::Unconfirmed;
        };
        if !LOG_WORKER.get() {
            let (tx, rx) = mpsc::sync_channel(1);
            if self.enqueue(
                Packet {
                    bytes: bytes.clone(),
                    receipt: Some(tx),
                },
                true,
            ) == Admission::Queued
                && rx.recv_timeout(FATAL_DELIVERY_TIMEOUT.saturating_sub(started.elapsed()))
                    == Ok(true)
            {
                return Receipt::DiskConfirmed;
            }
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.fallback.send(bytes, Some(tx), &self.shared.counters);
        if rx.recv_timeout(FATAL_BUDGET.saturating_sub(started.elapsed())) == Ok(true) {
            Receipt::StderrConfirmed
        } else {
            Receipt::Unconfirmed
        }
    }
}

const FATAL_DELIVERY_TIMEOUT: std::time::Duration = Duration::from_millis(200);
