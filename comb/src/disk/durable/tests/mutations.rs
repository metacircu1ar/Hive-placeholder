use super::*;
use crate::test_support::{TestComb, write as fixture_write};
use crate::{
    Actor, Comb,
    values::{ContentHash, Principal, Slug},
};

const BEFORE: &str = "---\ntitle: Page\n---\nBefore";
const AFTER: &str = "---\ntitle: Page\n---\nAfter";

struct Fixture {
    root: tempfile::TempDir,
    comb: Comb,
    recording: Arc<Recording>,
    slug: Slug,
    before: ContentHash,
}

impl Fixture {
    fn new() -> Self {
        let (root, mut comb) = TestComb::new().unwrap().into_parts();
        let slug = "concepts/page".parse().unwrap();
        let before = comb
            .put_markdown(
                &slug,
                BEFORE,
                None,
                &fixture_write(&Actor::Human, "seed", None).unwrap(),
            )
            .unwrap()
            .hash;
        let recording = Arc::new(Recording::default());
        recording.barrier(Ok(()));
        comb.disk.set_durability(recording.clone());
        Self {
            root,
            comb,
            recording,
            slug,
            before,
        }
    }
    fn mutate(&mut self) -> Result<crate::store::Mutation> {
        self.comb.put_markdown(
            &self.slug,
            AFTER,
            Some(&self.before),
            &fixture_write(&Actor::Human, "attempt", None).unwrap(),
        )
    }
    fn receipt(&self) -> Option<crate::page::Receipt> {
        self.comb
            .disk
            .receipt(
                &Principal::for_actor(&Actor::Human).unwrap(),
                &"attempt".parse().unwrap(),
            )
            .unwrap()
    }
    fn reopen(mut self, replayed: bool) {
        drop(self.comb);
        self.comb = Comb::open(self.root.path()).unwrap();
        assert!(!self.comb.disk.admin.join("intent.json").exists());
        assert_eq!(self.receipt().is_some(), replayed);
        let result = self.mutate().unwrap();
        assert_eq!(result.replayed, replayed);
        assert_eq!(result.request_id.as_str(), "attempt");
        assert_eq!(self.comb.get(&self.slug).unwrap().page.compiled, "After");
        assert_eq!(self.receipt().unwrap().request_id.as_str(), "attempt");
        assert!(self.mutate().unwrap().replayed);
    }
}

#[derive(Clone, Copy)]
enum AfterFailure {
    NoIntent,
    PageBefore,
    HeadBefore,
    ReceiptBefore,
    FinishPending,
    Finished,
}

fn assert_boundary(failure: (Operation, usize), expected: AfterFailure) {
    use AfterFailure::{FinishPending, Finished, HeadBefore, NoIntent, PageBefore, ReceiptBefore};
    let mut fixture = Fixture::new();
    fixture.recording.fail(failure.0, failure.1);
    let error = fixture.mutate().unwrap_err();
    assert_io(&error, !matches!(expected, NoIntent), None);
    let disk = &fixture.comb.disk;
    assert_eq!(
        disk.admin.join("intent.json").exists(),
        !matches!(expected, NoIntent | Finished)
    );
    assert_eq!(
        fixture.receipt().is_some(),
        matches!(expected, FinishPending | Finished)
    );
    let raw = disk.content(&fixture.slug).unwrap().unwrap();
    let renamed = !matches!(expected, NoIntent | PageBefore);
    assert_eq!(
        crate::Page::parse(&raw).unwrap().compiled,
        if renamed { "After" } else { "Before" }
    );
    let actual = ContentHash::digest(raw);
    if !renamed {
        assert_eq!(actual, fixture.before);
    }
    let head = match expected {
        NoIntent | PageBefore | HeadBefore => fixture.before.clone(),
        ReceiptBefore | FinishPending | Finished => actual,
    };
    assert_eq!(disk.head(&fixture.slug).unwrap(), Some(head));
    fixture.reopen(renamed);
}

#[test]
fn mutation_failures_keep_exact_metadata_evidence_and_replay() {
    use AfterFailure::{FinishPending, Finished, HeadBefore, NoIntent, PageBefore, ReceiptBefore};
    use Operation::{Fsync, FullFlush};
    for (operation, occurrence, expected) in [
        (Fsync, 1, NoIntent),
        (Fsync, 2, PageBefore),
        (FullFlush, 1, PageBefore),
        (Fsync, 3, PageBefore),
        (Fsync, 4, HeadBefore),
        (Fsync, 5, HeadBefore),
        (Fsync, 8, ReceiptBefore),
        (FullFlush, 2, FinishPending),
        (Fsync, 10, Finished),
    ] {
        assert_boundary((operation, occurrence), expected);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn mutation_ordering_errors_keep_intent_and_page_promotion_distinct() {
    for (occurrence, expected) in [(1, AfterFailure::NoIntent), (2, AfterFailure::PageBefore)] {
        assert_boundary((Operation::OrderingBarrier, occurrence), expected);
    }
}

#[test]
fn before_finish_retains_exact_error_and_durable_request() {
    let mut fixture = Fixture::new();
    fixture.recording.fail_finish_once();
    let error = fixture.mutate().unwrap_err();
    assert_eq!(error.code, ErrorCode::IoError);
    assert_eq!(error.message, "Injected finalization failure.");
    assert!(error.file_committed);
    assert!(error.request_id.is_none());
    assert!(fixture.comb.disk.admin.join("intent.json").exists());
    assert!(fixture.receipt().is_some());
    assert_eq!(
        fixture.recording.counts().events.last(),
        Some(&Operation::BeforeFinish)
    );
    assert_eq!(fixture.recording.counts().full_barriers, 1);
    fixture.reopen(true);
}
