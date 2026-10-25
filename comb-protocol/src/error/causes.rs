//! Resource-free causes for errors that own locks, queued jobs or panic payloads.
#[derive(Debug, thiserror::Error)]
pub enum Cause {
    #[error("lock poisoned")]
    Poisoned,
    #[error("channel full")]
    Full,
    #[error("channel closed")]
    Closed,
    #[error("path is not UTF-8")]
    NonUtf8Path,
    #[error("environment value missing")]
    MissingEnvironmentValue,
    #[error("environment value is not Unicode")]
    NonUnicodeEnvironmentValue,
    #[error("worker panicked")]
    Panicked,
    #[error("worker cancelled")]
    Cancelled,
}
impl Cause {
    pub fn environment(error: std::env::VarError) -> Self {
        // NotUnicode owns the rejected value, which may be a secret. Neither
        // Debug nor the retained source chain may expose that payload.
        match error {
            std::env::VarError::NotPresent => Self::MissingEnvironmentValue,
            std::env::VarError::NotUnicode(_) => Self::NonUnicodeEnvironmentValue,
        }
    }

    pub fn poisoned<T>(error: std::sync::PoisonError<T>) -> Self {
        drop(error); // A PoisonError owns its acquired guard; never retain it.
        Self::Poisoned
    }
    pub fn channel<T>(error: tokio::sync::mpsc::error::TrySendError<T>) -> Self {
        let cause = match &error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => Self::Full,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => Self::Closed,
        };
        drop(error); // Drop jobs, permits and waiting-count guards before returning.
        cause
    }
    pub fn non_utf8_path(error: std::ffi::OsString) -> Self {
        drop(error); // OsString has no Error implementation; the classification suffices.
        Self::NonUtf8Path
    }
    pub fn worker(error: tokio::task::JoinError) -> Self {
        let cause = if error.is_panic() {
            Self::Panicked
        } else {
            Self::Cancelled
        };
        drop(error); // Panic payloads may own arbitrary resources.
        cause
    }
}
