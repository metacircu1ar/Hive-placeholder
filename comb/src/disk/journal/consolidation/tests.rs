use super::*;
use crate::{
    Actor, Comb, ErrorCode,
    disk::{Operation, Recording, publication_hooks as hooks},
    store::{Consolidate, Consolidated, Write},
};
use atomic_fs::Step;
use std::{
    cell::RefCell,
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

mod concurrent;
mod faults;
mod process;
mod snapshots;
mod validation;

#[test]
fn malformed_accepted_pair_targets_fence_recovery_instead_of_being_quarantined() {
    for edit_note in [false, true] {
        let mut fixture = Fixture::new();
        fixture.pending();
        let slug = if edit_note {
            &fixture.request.note
        } else {
            &fixture.request.target
        };
        let path = fixture.root.path().join(format!("{slug}.md"));
        let intent = fixture.root.path().join(".comb/intent.json");
        let accepted = fs::read(&intent).unwrap();
        fs::write(
            &path,
            b"---\ntitle: [unfinished\n---\nexternal editor bytes",
        )
        .unwrap();
        let changed = fs::read(&path).unwrap();
        drop(fixture.comb);
        for _ in 0..2 {
            let error = Comb::open(fixture.root.path()).err().unwrap();
            assert!(error.file_committed);
            assert_eq!(error.code, ErrorCode::RecoveryRequired);
            assert_eq!(fs::read(&intent).unwrap(), accepted);
            assert_eq!(fs::read(&path).unwrap(), changed);
        }
    }
}

const STEPS: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

struct Fixture {
    root: tempfile::TempDir,
    comb: Comb,
    request: Consolidate,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut comb = Comb::init(root.path()).unwrap();
        let note = comb
            .capture("Original evidence", &write("capture"))
            .unwrap()
            .slug;
        let target = comb
            .put_markdown(
                &"concepts/target".parse().unwrap(),
                "---\ntitle: Target\n---\nOriginal summary",
                None,
                &write("target"),
            )
            .unwrap();
        let request = Consolidate {
            note_hash: comb.get(&note).unwrap().hash,
            note,
            target: target.slug,
            target_hash: target.hash,
            summary: "Combined summary".into(),
            recopy: false,
        };
        Self {
            root,
            comb,
            request,
        }
    }
    fn apply(&mut self) -> Result<Consolidated> {
        self.comb.consolidate(&self.request, &write("pair"))
    }
    fn pending(&mut self) -> Box<ConsolidationIntent> {
        let (result, pair) = self.capture(|path, step| {
            if path.file_name().unwrap() == "intent.json" && step == Step::AfterPublish {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        });
        assert!(result.unwrap_err().file_committed);
        pair.unwrap()
    }
    fn paths(&self) -> [PathBuf; 7] {
        let disk = &self.comb.disk;
        let note_id = RequestId::derived(
            crate::error::json_bytes(&(ReceiptOperation::ConsolidationSource, "pair")).unwrap(),
        );
        let principal = Actor::Human.principal().unwrap();
        [
            disk.admin.join("intent.json"),
            disk.page_path(&self.request.target, false).unwrap(),
            disk.head_path(&self.request.target),
            disk.receipt_path(principal.as_str(), "pair").unwrap(),
            disk.page_path(&self.request.note, false).unwrap(),
            disk.head_path(&self.request.note),
            disk.receipt_path(principal.as_str(), note_id.as_str())
                .unwrap(),
        ]
    }
    fn capture(
        &mut self,
        effect: impl Fn(&Path, Step) -> io::Result<()> + 'static,
    ) -> (Result<Consolidated>, Option<Box<ConsolidationIntent>>) {
        let pair = Rc::new(RefCell::new(None));
        let captured = pair.clone();
        let result = hooks::with(
            move |path, step| {
                if path.file_name().unwrap() == "intent.json" && step == Step::AfterPublish {
                    let record: super::super::Record =
                        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                    if let super::super::Record::Consolidation(value) = record {
                        *captured.borrow_mut() = Some(value);
                    }
                }
                effect(path, step)
            },
            || self.apply(),
        );
        let pair = pair.borrow_mut().take();
        (result, pair)
    }
    fn recover(self, pair: Option<Box<ConsolidationIntent>>) {
        drop(self.comb);
        let mut comb = Comb::open(self.root.path()).unwrap();
        if let Some(pair) = &pair {
            for part in [&pair.target, &pair.note] {
                assert_eq!(comb.disk.content(&part.slug).unwrap().unwrap(), part.raw);
                assert_eq!(comb.get(&part.slug).unwrap().hash, part.after);
                assert!(!comb.get(&part.slug).unwrap().unverified);
            }
        }
        let replay = comb.consolidate(&self.request, &write("pair")).unwrap();
        assert_eq!(replay.target.replayed, pair.is_some());
        assert_eq!(replay.note.replayed, pair.is_some());
        assert_eq!(
            comb.get(&self.request.target).unwrap().page.timeline.len(),
            1
        );
        assert_eq!(comb.stats().unwrap().inbox, 0);
        assert!(!comb.disk.admin.join("intent.json").exists());
    }
}
fn write(id: &str) -> Write<'static> {
    // Write owns validated request/run/principal state; the human actor is static.
    Write::new(&Actor::Human, &id.parse().unwrap(), Some("curation")).unwrap()
}
