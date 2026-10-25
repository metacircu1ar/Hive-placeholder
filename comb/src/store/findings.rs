//! Bound retained diagnostics without hiding errors from subsequent checks.
use super::Finding;

use super::limits::FINDING_LIMIT;

#[derive(Default)]
pub(super) struct Findings {
    retained: Vec<Finding>,
    truncated: bool,
}
impl Findings {
    pub(super) fn push(&mut self, finding: Finding) {
        if self.retained.len() < usize::from(FINDING_LIMIT) {
            self.retained.push(finding);
        } else {
            self.truncated = true;
        }
    }
    pub(super) fn extend(&mut self, findings: impl IntoIterator<Item = Finding>) {
        for finding in findings {
            self.push(finding);
        }
    }
    pub(super) fn into_parts(self) -> (Vec<Finding>, bool) {
        (self.retained, self.truncated)
    }
}
