//! Application checkpoints extend the generic filesystem publication boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkpoint {
    Publication(atomic_fs::Step),
    BeforeIntent,
    AfterIntent,
    /// A state record was opened for reading, before its metadata is checked.
    RecordOpened,
}
impl From<atomic_fs::Step> for Checkpoint {
    fn from(step: atomic_fs::Step) -> Self {
        Self::Publication(step)
    }
}
