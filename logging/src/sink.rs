//! Single-owned disk stream; normal records are regenerable diagnostics.
use crate::Config;
use atomic_fs::{Append, Durability, TailPolicy};
use std::{io, path::Path};

pub(crate) trait Sink: Send + 'static {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn finish(&mut self) -> io::Result<()>;
}
pub(crate) struct FileSink {
    config: Config,
    append: Append,
    needs_recovery: bool,
    // Drop after Append closes, even when finish times out and detaches the writer.
    _owner: Box<dyn Send>,
}
impl FileSink {
    #[cfg(test)]
    pub fn open(path: &Path, config: &Config) -> io::Result<Self> {
        Self::open_owned(path, config, Box::new(()))
    }
    pub fn open_owned(path: &Path, config: &Config, owner: Box<dyn Send>) -> io::Result<Self> {
        let append = Append::open(
            path,
            TailPolicy::rotate(
                std::num::NonZeroUsize::new(config.files - 1).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Rotation retention must be positive",
                    )
                })?,
            ),
            Durability::AtomicVisible,
        )?;
        Ok(Self {
            config: config.clone(),
            append,
            needs_recovery: false,
            _owner: owner,
        })
    }
    fn write_inner(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.needs_recovery {
            self.append.recover()?;
            self.needs_recovery = false;
        }
        let writer = &mut self.append;
        if writer.size()?.saturating_add(bytes.len() as u64) > self.config.file_bytes {
            writer.rotate()?;
        }
        writer.append_line(bytes.strip_suffix(b"\n").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Diagnostic record needs a final newline",
            )
        })?)
    }
}
impl Sink for FileSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let result = self.write_inner(bytes);
        if result.is_err() {
            // Retain the ownership lock. Never replay this uncertain event.
            self.needs_recovery = true;
        }
        result
    }
    fn finish(&mut self) -> io::Result<()> {
        if self.needs_recovery {
            return Err(io::Error::other("Unconfirmed diagnostic stream"));
        }
        self.append.sync()
    }
}
