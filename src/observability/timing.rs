//! Opt-in monotonic intervals; INFO does not read timing clocks or allocate records.
use std::time::{Duration, Instant};

pub(crate) struct Timer(Option<Start>);
struct Start {
    instant: Instant,
    monotonic: Option<u64>,
    cpu: Option<u64>,
}
pub(crate) struct Measurement {
    pub duration: u64,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub cpu: Option<u64>,
}
impl Timer {
    pub fn start() -> Self {
        Self(tracing::enabled!(tracing::Level::DEBUG).then(|| Start {
            instant: Instant::now(),
            monotonic: super::optional(crate::os::diagnostic_clock_ns(), "timing_start"),
            cpu: super::optional(crate::os::usage::thread_ns(), "timing_cpu_start"),
        }))
    }
    pub fn elapsed_ns(&self) -> u64 {
        self.0
            .as_ref()
            .map_or(0, |start| nanos(start.instant.elapsed()))
    }
    pub fn measure(self) -> Option<Measurement> {
        self.0.map(|start| Measurement {
            duration: nanos(start.instant.elapsed()),
            start: start.monotonic,
            end: super::optional(crate::os::diagnostic_clock_ns(), "timing_end"),
            cpu: start.cpu.and_then(|begin| {
                super::optional(crate::os::usage::thread_ns(), "timing_cpu_end")?.checked_sub(begin)
            }),
        })
    }
    pub fn finish(self, phase: &'static str, success: bool) {
        if let Some(m) = self.measure() {
            tracing::debug!(name: "runtime.timing", phase = phase, duration_ns = m.duration,
                start_ns = m.start, end_ns = m.end, cpu_ns = m.cpu, success = success);
        }
    }
}
pub(crate) fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}
