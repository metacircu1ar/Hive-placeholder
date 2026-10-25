use std::{collections::BTreeMap, io, time::Duration};
use tracing::level_filters::LevelFilter;

#[derive(Clone, Debug)]
pub struct Config {
    pub level: LevelFilter,
    /// Library overrides; otherwise third-party logs are capped at WARN.
    pub library_levels: BTreeMap<String, LevelFilter>,
    pub queue_slots: usize,
    pub record_bytes: usize,
    pub file_bytes: u64,
    pub files: usize,
    pub loss_interval: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            level: LevelFilter::INFO,
            library_levels: BTreeMap::new(),
            queue_slots: 1024,
            record_bytes: 8192,
            file_bytes: 10 * 1024 * 1024,
            files: 5,
            loss_interval: DEFAULT_LOSS_INTERVAL,
        }
    }
}
impl Config {
    /// Check all limits without opening files or starting worker threads.
    pub fn validate(&self) -> io::Result<()> {
        let capacity = self.queue_slots.checked_mul(self.record_bytes);
        if self.library_levels.len() > 32
            || self
                .library_levels
                .keys()
                .any(|target| !crate::policy::target_name(target))
            || !(1..=65536).contains(&self.queue_slots)
            || !(1024..=1024 * 1024).contains(&self.record_bytes)
            || capacity.is_none_or(|size| size > 16 * 1024 * 1024)
            || !(2..=64).contains(&self.files)
            || self.file_bytes < (self.record_bytes + atomic_fs::ROTATION_RECORD_LIMIT) as u64
            || self.file_bytes > 1024 * 1024 * 1024
            || !(MIN_LOSS_INTERVAL..=MAX_LOSS_INTERVAL).contains(&self.loss_interval)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid bounded logger limits",
            ));
        }
        Ok(())
    }
}

const DEFAULT_LOSS_INTERVAL: std::time::Duration = Duration::from_secs(1);

const MIN_LOSS_INTERVAL: std::time::Duration = Duration::from_millis(1);

const MAX_LOSS_INTERVAL: std::time::Duration = Duration::from_secs(60);
