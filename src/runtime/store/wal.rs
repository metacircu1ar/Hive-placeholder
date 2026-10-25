//! The writer's WAL policy. SQLite's automatic checkpoint copies pages inside
//! COMMIT, before the batch's replies. The writer's hook only records the WAL
//! size; the writer checkpoints after its replies, on the same thread and with
//! the same idle WAL, so the WAL still restarts from its first frame.
use rusqlite::{Connection, hooks::Wal};
use std::cell::Cell;

/// SQLite's default automatic-checkpoint threshold, in frames.
pub(in crate::runtime) const THRESHOLD: i32 = 1000;
/// With a background checkpointer, the writer waits briefly at this size for a
/// complete pass, so that its next write restarts the WAL (16 MiB of 4 KiB pages).
pub(in crate::runtime) const RESTART: i32 = 4 * THRESHOLD;
/// The longest the writer waits for that pass; a stalled sync is left behind.
pub(in crate::runtime) const RESTART_WAIT: std::time::Duration =
    std::time::Duration::from_millis(25);
/// With a background checkpointer, the writer checkpoints by itself at this
/// size (256 MiB of 4 KiB pages). At about 23,000 frames a second, that rides
/// out a checkpoint stall of nearly 3 s.
pub(in crate::runtime) const FALLBACK: i32 = 64 * 1024;
thread_local! {
    // The writer thread owns its connection, its commits and therefore its hook calls.
    static FRAMES: Cell<i32> = const { Cell::new(0) };
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "rusqlite's WAL hook signature returns a Result"
)]
fn hook(_: &Wal, frames: i32) -> rusqlite::Result<()> {
    FRAMES.with(|count| count.set(frames));
    Ok(())
}
/// Replace SQLite's automatic checkpoint on the writer's connection. Setting the
/// pragma clears any installed hook, so it comes first.
pub(in crate::runtime) fn install(connection: &Connection) -> rusqlite::Result<()> {
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.wal_hook(Some(hook));
    Ok(())
}
/// WAL frames after the writer thread's last commit that appended any.
pub(in crate::runtime) fn frames() -> i32 {
    FRAMES.with(Cell::get)
}
/// A checkpoint that copied every frame lets the next write restart the WAL. A
/// commit that appends nothing does not call the hook, so the count resets here.
pub(in crate::runtime) fn checkpointed(complete: bool) {
    if complete {
        FRAMES.with(|count| count.set(0));
    }
}
