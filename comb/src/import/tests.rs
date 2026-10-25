use super::*;
use crate::{
    Actor, ErrorCode,
    disk::{Operation, Recording, publication_hooks as hooks},
    store::Write,
    values::ImportId,
};
use atomic_fs::Step;
use comb_protocol::import::{ImportProgress, ImportState};
use std::{cell::Cell, fs, io, path::PathBuf, sync::Arc};

mod faults;
mod process;
mod retry;
mod scenarios;
mod validation;

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
    comb: Comb,
    root: tempfile::TempDir,
    source: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            fs::write(source.path().join(format!("{name}.md")), name).unwrap();
        }
        Self {
            comb: Comb::init(root.path()).unwrap(),
            root,
            source,
        }
    }
    fn apply(&mut self) -> Result<ImportReport> {
        self.comb.import(self.source.path(), None)
    }
    fn status(&self) -> ImportProgress {
        self.comb.import_status().unwrap().current.unwrap()
    }
    fn pending(&mut self, completed: usize) -> ImportProgress {
        let error = self.fail("intent.json", Step::BeforePublish, completed + 1);
        let progress = error.import_progress.unwrap();
        assert!(error.file_committed);
        assert_eq!(progress.completed, completed);
        assert_eq!(progress.unfinished, 3 - completed);
        self.comb.ready().unwrap();
        *progress
    }
    fn fail(&mut self, file: &str, step: Step, occurrence: usize) -> Error {
        let file = file.to_owned();
        let count = Cell::new(0);
        hooks::with(
            move |path, at| {
                if path.file_name().unwrap() == file.as_str() && at == step {
                    count.set(count.get() + 1);
                    if count.get() == occurrence {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                }
                Ok(())
            },
            || self.apply().unwrap_err(),
        )
    }
    fn reopen(self) -> Self {
        drop(self.comb);
        Self {
            comb: Comb::open(self.root.path()).unwrap(),
            root: self.root,
            source: self.source,
        }
    }
    fn journal(&self) -> Vec<u8> {
        fs::read(self.root.path().join(".comb/import.json")).unwrap()
    }
    fn page(&self, name: &str) -> PathBuf {
        self.root.path().join(format!("{name}.md"))
    }
}
fn write(id: &str) -> Write<'static> {
    Write::new(&Actor::Human, &id.parse().unwrap(), None).unwrap()
}
