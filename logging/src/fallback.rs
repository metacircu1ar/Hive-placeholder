//! Stderr can block too: isolate it behind one prestarted, bounded worker.
use crate::Counters;
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

pub(crate) struct Notice {
    pub bytes: Vec<u8>,
    pub receipt: Option<SyncSender<bool>>,
}
enum Message {
    Record(Notice),
    Close,
}
#[derive(Clone)]
pub(crate) struct Fallback {
    sender: SyncSender<Message>,
    closing: Arc<AtomicBool>,
}
impl Fallback {
    pub fn start(
        counters: Arc<Counters>,
        output: Box<dyn Write + Send>,
        spawn: &impl crate::Spawn,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(4);
        let closing = Arc::new(AtomicBool::new(false));
        let stop = closing.clone();
        // Dropping the handle deliberately detaches a potentially blocked stderr.
        spawn.spawn(
            "hive-log-stderr",
            Box::new(move || run(receiver, stop, counters, output)),
        )?;
        Ok(Self { sender, closing })
    }
    pub fn send(&self, bytes: Vec<u8>, receipt: Option<SyncSender<bool>>, counters: &Counters) {
        if self.closing.load(Ordering::Acquire)
            || self
                .sender
                .try_send(Message::Record(Notice { bytes, receipt }))
                .is_err()
        {
            counters.fallback_failed.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn close(&self) {
        self.closing.store(true, Ordering::Release);
        // Full means queued work already wakes recv; it will observe closing
        // before waiting again. Disconnected means the worker already ended.
        let _ = self.sender.try_send(Message::Close);
    }
}
fn run(
    receiver: Receiver<Message>,
    closing: Arc<AtomicBool>,
    counters: Arc<Counters>,
    mut output: Box<dyn Write + Send>,
) {
    loop {
        #[cfg(test)]
        counters.fallback_iterations.fetch_add(1, Ordering::Relaxed);
        let message = if closing.load(Ordering::Acquire) {
            match receiver.try_recv() {
                Ok(n) => n,
                Err(_) => break,
            }
        } else {
            match receiver.recv() {
                Ok(n) => n,
                Err(_) => break,
            }
        };
        let Message::Record(notice) = message else {
            continue;
        };
        let confirmed = output
            .write_all(&notice.bytes)
            .and_then(|()| output.flush())
            .is_ok();
        if !confirmed {
            counters.fallback_failed.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(receipt) = notice.receipt {
            let _ = receipt.try_send(confirmed);
        }
    }
}

pub(crate) trait Spawn {
    fn spawn(
        &self,
        name: &'static str,
        task: Box<dyn FnOnce() + Send>,
    ) -> io::Result<thread::JoinHandle<()>>;
}
pub(crate) struct Threads;
impl Spawn for Threads {
    fn spawn(
        &self,
        name: &'static str,
        task: Box<dyn FnOnce() + Send>,
    ) -> io::Result<thread::JoinHandle<()>> {
        thread::Builder::new().name(name.into()).spawn(task)
    }
}
