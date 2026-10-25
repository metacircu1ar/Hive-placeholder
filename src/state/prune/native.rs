//! Delete only the observed native item; retain identity across crash/retry.
use crate::{
    error::{Error, Result},
    state::{self, generations::Generation, journal},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(super) trait Items {
    fn exists(&self, reference: &[u8]) -> Result<bool>;
    fn lookup(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>>;
    fn delete(&self, service: &str, account: &str, reference: &[u8]) -> Result<()>;
}
#[cfg(target_os = "macos")]
struct Keychain;
#[cfg(target_os = "macos")]
impl Items for Keychain {
    fn exists(&self, reference: &[u8]) -> Result<bool> {
        crate::os::keychain::exists(reference).map_err(Error::state_io_from)
    }
    fn lookup(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>> {
        crate::os::keychain::lookup(service, account).map_err(Error::state_io_from)
    }
    fn delete(&self, service: &str, account: &str, reference: &[u8]) -> Result<()> {
        crate::os::keychain::delete(service, account, reference).map_err(Error::state_io_from)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Removal {
    generation: uuid::Uuid,
    service: String,
    account: String,
    reference: Vec<u8>,
}
fn refused() -> Error {
    Error::invalid_request(
        "The expected Claude Keychain item is missing or changed; its profile and \
            credentials were retained. Inspect the recorded account and native item \
            before pruning.",
    )
}
#[cfg(target_os = "macos")]
pub(super) fn remove(generation: &Generation) -> Result<()> {
    remove_with(generation, &Keychain)
}
fn read(
    path: &Path,
    generation: &Generation,
    service: &str,
    account: &str,
) -> Result<Option<Removal>> {
    if !state::recovery::exists(path)? {
        return Ok(None);
    }
    let record: Removal = state::read_json(path)?;
    if record.generation != generation.binding.id
        || record.service != service
        || record.account != account
        || record.reference.is_empty()
        || record.reference.len() > 4096
    {
        return Err(state::corrupt());
    }
    journal::sync_record(path)?;
    Ok(Some(record))
}
pub(super) fn remove_with(generation: &Generation, items: &impl Items) -> Result<()> {
    let account = generation
        .keychain_account
        .as_deref()
        .ok_or_else(state::corrupt)?;
    let service =
        state::generations::claude_namespace(&generation.binding.directory.join("provider"))?;
    if generation
        .credential_namespace
        .as_deref()
        .is_some_and(|saved| saved != service)
    {
        return Err(state::corrupt());
    }
    let path = generation.binding.directory.join("native-removal.json");
    let prior = read(&path, generation, &service, account)?;
    let observed = items.lookup(&service, account)?;
    let record = match (prior, observed) {
        (Some(record), None) => return absent(items, &record.reference),
        (Some(record), Some(reference)) if record.reference == reference => record,
        (None, Some(reference)) if !reference.is_empty() && reference.len() <= 4096 => {
            let record = Removal {
                generation: generation.binding.id,
                service,
                account: account.into(),
                reference,
            };
            journal::publish(&path, &record)?;
            record
        }
        _ => return Err(refused()),
    };
    // Check again after durable intent. Delete by persistent reference and exact
    // service/account, so replacement between this check and delete is untouched.
    match items.lookup(&record.service, &record.account)? {
        None => return absent(items, &record.reference),
        Some(reference) if reference == record.reference => {}
        Some(_) => return Err(refused()),
    }
    items.delete(&record.service, &record.account, &record.reference)?;
    if items.lookup(&record.service, &record.account)?.is_some() {
        return Err(refused());
    }
    absent(items, &record.reference)
}
fn absent(items: &impl Items, reference: &[u8]) -> Result<()> {
    if items.exists(reference)? {
        Err(refused())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
