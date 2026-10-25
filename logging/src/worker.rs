use crate::{
    Counts, Fields, Level, Shutdown,
    fallback::Fallback,
    producer::{LOG_WORKER, Packet, Shared},
    record::encode,
    sink::Sink,
};
use std::{
    io,
    sync::{Arc, atomic::Ordering, mpsc::Receiver},
    thread,
    time::{Duration, Instant},
};

struct Worker {
    shared: Arc<Shared>,
    sink: Box<dyn Sink>,
    fallback: Fallback,
    retry_at: Instant,
    failed: bool,
}
pub(crate) fn run(
    shared: Arc<Shared>,
    normal: Receiver<Packet>,
    priority: Receiver<Packet>,
    sink: Box<dyn Sink>,
    fallback: Fallback,
) -> Shutdown {
    LOG_WORKER.set(true);
    let mut writer = Worker {
        shared,
        sink,
        fallback,
        retry_at: Instant::now(),
        failed: false,
    };
    let mut reported = Counts::default();
    let mut next_receipt = Instant::now() + writer.shared.config.loss_interval;
    loop {
        #[cfg(test)]
        writer
            .shared
            .counters
            .writer_iterations
            .fetch_add(1, Ordering::Relaxed);
        if Instant::now() >= next_receipt {
            writer.report_loss(&mut reported);
            next_receipt = Instant::now() + writer.shared.config.loss_interval;
        }
        if let Ok(packet) = priority.try_recv().or_else(|_| normal.try_recv()) {
            let confirmed = writer.write(&packet.bytes, packet.receipt.is_some());
            if let Some(receipt) = packet.receipt {
                let _ = receipt.try_send(confirmed);
            }
            continue;
        }
        if writer.shared.closing.load(Ordering::SeqCst)
            && writer.shared.admitting.load(Ordering::SeqCst) == 0
        {
            // The previous empty check could precede a producer's final enqueue.
            if let Ok(packet) = priority.try_recv().or_else(|_| normal.try_recv()) {
                let confirmed = writer.write(&packet.bytes, packet.receipt.is_some());
                if let Some(receipt) = packet.receipt {
                    let _ = receipt.try_send(confirmed);
                }
                continue;
            }
            break;
        }
        thread::park_timeout(next_receipt.saturating_duration_since(Instant::now()));
    }
    writer.report_loss(&mut reported);
    if let Err(error) = writer.sink.finish() {
        writer.failure(&error, false);
    }
    if writer.failed {
        Shutdown::WriteFailed
    } else {
        Shutdown::Drained
    }
}
impl Worker {
    fn write(&mut self, bytes: &[u8], flush: bool) -> bool {
        if Instant::now() < self.retry_at {
            self.shared
                .counters
                .write_failed
                .fetch_add(1, Ordering::Relaxed);
            self.failed = true;
            return false;
        }
        let result = self
            .sink
            .write(bytes)
            .and_then(|()| if flush { self.sink.finish() } else { Ok(()) });
        match result {
            Ok(()) => {
                self.shared.counters.written.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(error) => {
                self.failure(&error, true);
                false
            }
        }
    }
    fn failure(&mut self, error: &io::Error, record_failed: bool) {
        self.failed = true;
        let (counter, event) = if record_failed {
            (&self.shared.counters.write_failed, "log.write_failed")
        } else {
            (&self.shared.counters.finish_failed, "log.finish_failed")
        };
        counter.fetch_add(1, Ordering::Relaxed);
        self.shared
            .counters
            .errno
            .store(error.raw_os_error().unwrap_or(0), Ordering::Relaxed);
        self.retry_at = Instant::now() + FAILURE_REPORT_INTERVAL;
        let mut fields = Fields::default();
        let bytes = fields
            .cause(error)
            .and_then(|()| encode(Level::Error, event, fields, self.shared.config.record_bytes));
        match bytes {
            Ok(bytes) => self.fallback.send(bytes, None, &self.shared.counters),
            Err(_) => {
                self.shared.counters.invalid.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    fn report_loss(&mut self, reported: &mut Counts) {
        let mut counts = self.shared.counters.snapshot();
        // Successful records alone must not cause a self-sustaining receipt loop.
        counts.written = 0;
        if counts == *reported || Instant::now() < self.retry_at {
            return;
        }
        let mut fields = Fields::default();
        let encoded = (|| {
            fields.count("full", counts.full)?;
            fields.count("fatal_unconfirmed", counts.fatal_unconfirmed)?;
            fields.count("disconnected", counts.disconnected)?;
            fields.count("invalid", counts.invalid)?;
            fields.count("oversized", counts.oversized)?;
            fields.count("numeric_invalid", counts.numeric_invalid)?;
            fields.count("write_failed", counts.write_failed)?;
            fields.count("finish_failed", counts.finish_failed)?;
            fields.count("fallback_failed", counts.fallback_failed)?;
            fields.count(
                "filesystem_cleanup_failed",
                counts.filesystem_cleanup_failed,
            )?;
            encode(
                Level::Warn,
                "log.dropped",
                fields,
                self.shared.config.record_bytes,
            )
        })();
        match encoded {
            Ok(bytes) if self.write(&bytes, false) => *reported = counts,
            Ok(_) => {}
            Err(_) => {
                self.shared.counters.invalid.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

const FAILURE_REPORT_INTERVAL: std::time::Duration = Duration::from_secs(1);
