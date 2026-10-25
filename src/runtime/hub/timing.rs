//! One bounded DEBUG record per writer batch, emitted after replies/checkpoint.
use crate::observability::timing::Timer;

pub(super) struct Batch {
    timer: Timer,
    pub jobs: usize,
    pub queue_wait_ns: u64,
    pub max_queue_wait_ns: u64,
    pub apply_ns: u64,
    pub commit_ns: u64,
    pub checkpoint_ns: u64,
    /// `none`, `background`, `restart`, `deferred` or `idle`.
    pub checkpoint: &'static str,
    /// WAL frames after this batch's commit (SQLite's count; it restarts at 0).
    pub frames: i32,
}
impl Batch {
    pub fn start() -> Self {
        Self {
            timer: Timer::start(),
            jobs: 0,
            queue_wait_ns: 0,
            max_queue_wait_ns: 0,
            apply_ns: 0,
            commit_ns: 0,
            checkpoint_ns: 0,
            checkpoint: "none",
            frames: 0,
        }
    }
    pub fn received(&mut self, queued: Timer) {
        let wait = queued.elapsed_ns();
        self.queue_wait_ns = self.queue_wait_ns.saturating_add(wait);
        self.max_queue_wait_ns = self.max_queue_wait_ns.max(wait);
        self.jobs += 1;
    }
    pub fn finish(self, outcome: &'static str, success: bool) {
        if let Some(m) = self.timer.measure() {
            tracing::debug!(name: "writer.batch", phase = outcome, success = success,
                jobs = self.jobs, queue_wait_ns = self.queue_wait_ns,
                max_queue_wait_ns = self.max_queue_wait_ns, apply_ns = self.apply_ns,
                commit_ns = self.commit_ns, checkpoint = self.checkpoint,
                checkpoint_ns = self.checkpoint_ns, frames = self.frames, duration_ns = m.duration,
                start_ns = m.start, end_ns = m.end, cpu_ns = m.cpu);
        }
    }
}
