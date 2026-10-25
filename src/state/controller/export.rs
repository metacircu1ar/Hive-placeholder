//! Export retained controller rows and independently verified ownership evidence.
use super::{
    State, check_path, corrupt, queries, read_json,
    snapshot::{fingerprint, snapshot_database},
};
use crate::{
    error::{Error, Result},
    runtime::api::value as project,
};
use rusqlite::Connection;
use serde_json::{Map, Value};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    str,
};
use uuid::Uuid;
#[cfg(test)]
mod tests;
mod wire;

// The diagnostic JSON represents each binary byte as two hexadecimal digits.
const HEX_DIGITS_PER_BYTE: usize = 2;

impl State {
    pub fn export_retired(&self, destination: &Path) -> Result<Value> {
        self.export_retired_with(destination, &atomic_fs::DefaultBarriers)
    }

    fn export_retired_with(
        &self,
        destination: &Path,
        barriers: &impl atomic_fs::Barriers,
    ) -> Result<Value> {
        hive_setup_support::names::check_output_name(destination).map_err(|cause| {
            Error::invalid_path("Export filename is reserved for Hive state preparations.")
                .with_source(cause)
        })?;
        let source = self.retired_controller()?.ok_or(Error::invalid_request(
            "No retired controller is recorded in this root.",
        ))?;
        let source = source.canonicalize().map_err(Error::state_io_from)?;
        check_path(&source, true)?;
        let parent = destination
            .parent()
            .ok_or_else(corrupt)?
            .canonicalize()
            .map_err(Error::state_io_from)?;
        if !destination.is_absolute()
            || parent.starts_with(&source)
            || crate::state::recovery::exists(destination)?
        {
            return Err(Error::invalid_path(
                "Export must be a new absolute file outside the retired root.",
            ));
        }
        let target = parent.join(destination.file_name().ok_or_else(corrupt)?);
        let snapshot = snapshot_database(&source.join("runtime/runtime.sqlite"))?;
        // Only the disposable copy is opened by SQLite. It may rebuild its own
        // SHM/replay WAL; the old root is never opened by SQLite or checkpointed.
        let db = Connection::open(snapshot.path().join("runtime.sqlite"))
            .map_err(|e| corrupt().with_source(e))?;
        let integrity: String = db
            .query_row(queries::EXPORT_RETIRED, [], |row| row.get(0))
            .map_err(|e| corrupt().with_source(e))?;
        if integrity != "ok" {
            return Err(corrupt());
        }
        let manifest = project(wire::Manifest {
            kind: "manifest",
            source: &source,
            consistent_snapshot: true,
            format: hive_input::compatibility::Exact,
            encoding: "ndjson",
        });
        let output =
            atomic_fs::PreparedCreate::new(&target, hive_setup_support::names::PUBLICATION_OPTIONS)
                .map_err(crate::publication::error)?;
        let (count, evidence) = output
            .publish_stream(
                |mut output| {
                    writeln!(output, "{manifest}")?;
                    let count = export_rows(&db, &mut output).map_err(io::Error::other)?;
                    // Evidence is independent per record, not atomic with the database.
                    let evidence =
                        export_evidence(&source, &mut output).map_err(io::Error::other)?;
                    Ok((count, evidence))
                },
                barriers,
            )
            .map_err(crate::publication::error)?;
        Ok(project(wire::Report {
            operation: "history_export",
            source: &source,
            output: destination,
            rows: count,
            ownership_evidence: evidence,
            large_output_files: "retained in the original root; not included",
        }))
    }
}

fn export_evidence(source: &Path, mut output: impl Write) -> Result<wire::EvidenceSummary> {
    let profiles = source.join("profiles");
    let mut export = EvidenceExport {
        source,
        output: &mut output,
        counts: (0_u64, 0_u64),
    };
    if profiles.try_exists().map_err(Error::state_io_from)? {
        check_path(&profiles, true)?;
        for profile in fs::read_dir(&profiles).map_err(Error::state_io_from)? {
            let profile = profile.map_err(Error::state_io_from)?;
            if profile
                .file_name()
                .to_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .is_none()
            {
                continue;
            }
            let dir = profile.path();
            if let Err(error) = check_path(&dir, true) {
                export.emit(&dir, Some(error))?;
                continue;
            }
            for name in [
                "identity.json",
                "prune-intent.json",
                "prune-receipt.json",
                "native-removal.json",
            ] {
                let path = dir.join(name);
                match crate::state::recovery::exists(&path) {
                    Ok(true) => export.emit(&path, None)?,
                    Ok(false) => {}
                    Err(error) => export.emit(&path, Some(error))?,
                }
            }
            // Never traverse credential or native workspace directories.
            for name in [
                "executions",
                "writers",
                "execution-archive",
                "recovery-receipts",
                "credential-resolutions",
            ] {
                let base = dir.join(name);
                if !base.try_exists().map_err(Error::state_io_from)? {
                    continue;
                }
                if let Err(error) = check_path(&base, true) {
                    export.emit(&base, Some(error))?;
                    continue;
                }
                for entry in fs::read_dir(base).map_err(Error::state_io_from)? {
                    let path = entry.map_err(Error::state_io_from)?.path();
                    export.emit(&path, None)?;
                }
            }
        }
    }
    Ok(wire::EvidenceSummary {
        records: export.counts.0,
        unavailable: export.counts.1,
        consistency: "each record checked independently; not atomic with database",
    })
}
pub(super) struct ExportPath<'a> {
    pub(super) diagnostic: std::borrow::Cow<'a, str>,
    pub(super) bytes: Option<&'a [u8]>,
}
pub(super) fn export_path(path: &Path) -> ExportPath<'_> {
    use std::os::unix::ffi::OsStrExt;
    ExportPath {
        diagnostic: path.to_string_lossy(),
        bytes: path.to_str().is_none().then(|| path.as_os_str().as_bytes()),
    }
}
fn export_rows(db: &Connection, mut output: impl Write) -> Result<u64> {
    let mut count = 0_u64;
    for table in ["agents", "messages", "events"] {
        let mut statement = db
            .prepare(&queries::table_rows(table))
            .map_err(|e| corrupt().with_source(e))?;
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|s| (*s).into())
            .collect();
        let mut rows = statement.query([]).map_err(|e| corrupt().with_source(e))?;
        while let Some(row) = rows.next().map_err(|e| corrupt().with_source(e))? {
            let mut value = Map::new();
            for (i, column) in columns.iter().enumerate() {
                use rusqlite::types::ValueRef;
                let v = match row.get_ref(i).map_err(|e| corrupt().with_source(e))? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => v.into(),
                    ValueRef::Real(v) => v.into(),
                    ValueRef::Text(v) => str::from_utf8(v)
                        .map_err(|e| corrupt().with_source(e))?
                        .into(),
                    ValueRef::Blob(bytes) => {
                        use std::fmt::Write as _;
                        let mut data = String::with_capacity(bytes.len() * HEX_DIGITS_PER_BYTE);
                        for byte in bytes {
                            write!(&mut data, "{byte:02x}")
                                .map_err(|e| corrupt().with_source(e))?;
                        }
                        project(wire::Blob {
                            encoding: "hex",
                            data,
                        })
                    }
                };
                value.insert(column.clone(), v);
            }
            writeln!(output, "{}", project(wire::Row { table, row: value }))
                .map_err(Error::state_io_from)?;
            count += 1;
        }
    }

    Ok(count)
}

struct EvidenceExport<'a, W> {
    source: &'a Path,
    output: &'a mut W,
    counts: (u64, u64),
}
impl<W: Write> EvidenceExport<'_, W> {
    fn emit(&mut self, path: &Path, unavailable: Option<Error>) -> Result<()> {
        let record = unavailable.map_or_else(
            || {
                let before = fingerprint(path)?;
                let record: Value = read_json(path)?;
                if before != fingerprint(path)? {
                    return Err(Error::recovery_required("Evidence changed during export."));
                }
                Ok(record)
            },
            Err,
        );
        let path = path
            .strip_prefix(self.source)
            .map_err(|e| corrupt().with_source(e))?;
        let ExportPath {
            diagnostic: path,
            bytes: raw_path,
        } = export_path(path);
        let outcome = match record {
            Ok(record) => {
                self.counts.0 += 1;
                wire::EvidenceOutcome::Record { record }
            }
            Err(error) => {
                self.counts.1 += 1;
                wire::EvidenceOutcome::Unavailable {
                    unavailable: error.to_string(),
                }
            }
        };
        let row = project(wire::Evidence {
            kind: "ownership_evidence",
            path,
            path_bytes: raw_path,
            outcome,
        });
        writeln!(self.output, "{row}").map_err(Error::state_io_from)
    }
}
