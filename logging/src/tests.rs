use crate::{Config, Guard, Logger, fallback::Threads, guard::start_with, sink::Sink};
use std::{
    io::{self, Write},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

mod files;
mod metadata;
mod panic;
mod reader;
mod runtime;
mod stderr;

#[derive(Clone, Default)]
struct Memory(Arc<Mutex<Vec<Vec<u8>>>>);
impl Memory {
    fn records(&self) -> Vec<serde_json::Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|b| serde_json::from_slice(b).unwrap())
            .collect()
    }
}
impl Sink for Memory {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Write for Memory {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Sink::write(self, bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Blocked {
    inner: Box<dyn Sink>,
    entered: SyncSender<()>,
    release: Receiver<()>,
    once: bool,
}
impl Blocked {
    fn new(inner: impl Sink) -> (Self, Receiver<()>, SyncSender<()>) {
        let (entered, notified) = mpsc::sync_channel(1);
        let (release, waiting) = mpsc::sync_channel(1);
        (
            Self {
                inner: Box::new(inner),
                entered,
                release: waiting,
                once: false,
            },
            notified,
            release,
        )
    }
}
impl Sink for Blocked {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !self.once {
            self.once = true;
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
        }
        self.inner.write(bytes)
    }
    fn finish(&mut self) -> io::Result<()> {
        self.inner.finish()
    }
}
impl Write for Blocked {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Sink::write(self, bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn start_memory(config: Config) -> (Logger, Guard, Memory) {
    let memory = Memory::default();
    let (logger, guard) = start_with(
        config,
        Box::new(memory.clone()),
        Box::new(io::sink()),
        &Threads,
    )
    .unwrap();
    (logger, guard, memory)
}
fn until(mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "condition did not become true"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

mod tracing_api;
