//! A fresh root remembers setup's choice before initialization can be interrupted.
//! Accepted embedding activation then uses the root's own transition journal.
use super::{Path, PathBuf, Plan, Result, files, invalid};
use serde::{Deserialize, Serialize};
use std::fs;
const NAME: &str = ".comb-setup.json";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    root: PathBuf,
    pub socket: PathBuf,
    /// The HTTP embedding endpoint, or none for text search only.
    pub http: Option<crate::embedding::HttpConfig>,
}
impl Intent {
    pub(super) fn check(&self, plan: &Plan) -> Result<()> {
        if self.socket != plan.socket || plan.http.is_some() && plan.http != self.http {
            return Err(invalid(
                "Another fresh setup choice is pending; retry with omitted selection flags.",
            ));
        }
        Ok(())
    }
}
pub(super) fn read(root: &Path) -> Result<Option<Intent>> {
    let path = root.join(NAME);
    if !crate::disk::exists(&path)? {
        return Ok(None);
    }
    let intent: Intent = files::json(&path)?;
    if intent.root != root || !intent.socket.is_absolute() {
        return Err(invalid(
            "Invalid fresh-root setup intent; inspect .comb-setup.json.",
        ));
    }
    if let Some(http) = &intent.http {
        crate::embedding::Engine::http_identity(http.clone())?;
    }
    Ok(Some(intent))
}
pub(super) fn publish(plan: &Plan) -> Result<()> {
    let intent = Intent {
        root: plan.root.clone(),
        socket: plan.socket.clone(),
        http: plan.http.clone(),
    };
    let bytes = files::encode_record(&intent)?;
    let _scope = hive_setup_support::locks::path(&plan.root, "comb-root", true)?;
    if crate::disk::exists(&plan.root.join(".comb"))? {
        return Err(invalid(
            "Root was initialized during setup; retry to inspect its current settings.",
        ));
    }
    if let Some(existing) = read(&plan.root)? {
        return if existing == intent {
            files::sync_json(&plan.root.join(NAME), &intent)
        } else {
            Err(invalid(
                "Another fresh-root setup choice is pending; inspect .comb-setup.json.",
            ))
        };
    }
    files::create_record(&plan.root.join(NAME), &bytes)
}
pub(super) fn clear(root: &Path) -> Result<()> {
    match fs::remove_file(root.join(NAME)) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    // Absence may be a retry after unlink succeeded but its barrier failed.
    #[cfg(test)]
    files::hooks::step(&root.join(NAME), atomic_fs::Step::BeforeDirectorySync)
        .map_err(crate::Error::from)
        .map_err(crate::Error::committed)?;
    fs::File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(crate::Error::from)
        .map_err(crate::Error::committed)
}
