//! Stream the artifact index without buffering the entire manifest in memory.
use super::{Store, database, rows, shared_queries};
use crate::{
    error::{Error, Result},
    runtime::api,
    state,
};
use atomic_fs::Barriers;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

impl Store {
    pub(super) fn manifest(&self, runtime: &Path) -> Result<()> {
        state::check_path(runtime, true)?;
        let target = runtime.join("output-manifest.jsonl");
        match fs::symlink_metadata(&target) {
            Ok(_) => state::check_path(&target, false)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::state_io_from(error)),
        }
        atomic_fs::Publisher::new(
            (hive_setup_support::names::PUBLICATION_OPTIONS)
                .with_prefix(hive_setup_support::names::STATE_PREFIX),
        )
        .with_barriers(&ManifestBarriers { path: &target })
        .write_stream(&target, |writer| {
            self.write_manifest(writer).map_err(io::Error::other)
        })
        .map_err(Error::manifest_publication)
    }

    fn write_manifest(&self, writer: &mut dyn Write) -> Result<()> {
        self.sql()
            .map_error(database)
            .with_rows(shared_queries::ARTIFACT_MANIFEST, [], |rows| {
                while let Some(rows::Artifact {
                    id,
                    bytes,
                    created_at,
                }) = rows.next()?
                {
                    writeln!(
                        writer,
                        "{}",
                        api::value(api::events::ArtifactManifest {
                            id: &id,
                            bytes,
                            created_at,
                            path: format!("output/{id}.json"),
                        })
                    )
                    .map_err(Error::state_io_from)?;
                }
                Ok(())
            })
    }
}

struct ManifestBarriers<'a> {
    path: &'a Path,
}
impl Barriers for ManifestBarriers<'_> {
    fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
        #[cfg(test)]
        return state::sync_tests::step(self.path, step);
        #[cfg(not(test))]
        let _ = (self.path, step);
        #[cfg(not(test))]
        Ok(())
    }
}

#[cfg(test)]
mod tests;
