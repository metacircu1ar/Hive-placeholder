//! Standalone diagnostics share the bounded worker and sanitized record boundary.
use crate::sink::Sink;
use std::{fs::File, io, io::Write, os::fd::AsFd};

pub(crate) fn descriptor() -> io::Result<File> {
    // A blocked diagnostic writer must not hold std::io::stderr's global lock:
    // the main thread may still need to emit the primary command result.
    std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
}

pub(crate) struct StderrSink(pub Box<dyn Write + Send>);
impl Sink for StderrSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let line = human(bytes)?;
        // Never replay a partial diagnostic line into a potentially shared pipe.
        if self.0.write(line.as_bytes())? != line.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Short diagnostic line",
            ));
        }
        Ok(())
    }
    fn finish(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

fn human(bytes: &[u8]) -> io::Result<String> {
    // Only the backend's already bounded and sanitized JSON reaches this sink.
    let record: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut line = String::from("Hive ");
    for key in ["level", "event"] {
        let text = record[key]
            .as_str()
            .ok_or_else(|| io::Error::other("Missing diagnostic label"))?;
        line.push_str(text);
        line.push(' ');
    }
    if let Some(fields) = record.as_object() {
        for (key, value) in fields {
            if matches!(
                key.as_str(),
                "t" | "level" | "event" | "target" | "location" | "spans" | "spans_truncated"
            ) {
                continue;
            }
            line.push_str(key);
            line.push('=');
            line.push_str(&value.to_string());
            line.push(' ');
        }
    }
    line.pop();
    line.push('\n');
    Ok(line)
}
