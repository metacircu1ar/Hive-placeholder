//! Service-owned identity/capability bindings. No caller-selected human actor.
#[cfg(test)]
mod golden;
mod output;
mod permission;
pub use output::{CredentialOutput, has_cleanup_failure, with_cleanup};
#[cfg(test)]
mod poison_tests;
#[cfg(test)]
mod values_tests;
use super::limits::{REGISTRY_BYTES, REGISTRY_RECORDS};
use crate::error::CommitExt;
use crate::{
    Actor, Error, Result,
    disk::{self, Disk},
    locks::{Health, RwLock},
    values::{CombId, CredentialId, Principal, RequestId},
};
use hmac::{Hmac, Mac};
pub(crate) use permission::ReviewPermission;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize)]
struct Record {
    id: CredentialId,
    grant: Grant,
    verifier: [u8; 32],
}
#[derive(Clone, Default, Serialize)]
struct Records {
    credentials: Vec<Record>,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRecords {
    credentials: Vec<RawRecord>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    id: String,
    grant: Grant,
    verifier: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub id: CredentialId,
    pub grant: Grant,
}
pub(crate) struct Authority {
    actor: Actor,
    pub reviewer: bool,
    principal: Principal,
}
impl Authority {
    pub(crate) fn new(actor: Actor, reviewer: bool) -> Result<Self> {
        let principal = Principal::for_actor(&actor)?;
        Ok(Self {
            actor,
            reviewer,
            principal,
        })
    }
    pub(crate) fn write<'a>(
        &'a self,
        request_id: &RequestId,
        run: Option<&'a str>,
    ) -> crate::store::Write<'a> {
        crate::store::Write::authorized(self, request_id, run)
    }
    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }
    pub(crate) fn actor(&self) -> &Actor {
        &self.actor
    }
    pub fn human(&self) -> Result<()> {
        if self.actor.is_human() {
            Ok(())
        } else {
            Err(Error::forbidden())
        }
    }
}
pub(crate) struct Registry {
    durability: Arc<dyn disk::Durability>,
    comb_id: CombId,
    key: [u8; 32],
    path: PathBuf,
    records: RwLock<Records>,
    health: Arc<Health>,
    // SeqCst pairs durable-uncertainty publication with authorize/persist checks.
    // Monotonic per Registry; human authorization bypasses this flag, not Health.
    broken: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fence_after_atomic: std::sync::atomic::AtomicBool,
}
impl Registry {
    pub fn open(disk: &Disk, health: Arc<Health>) -> Result<Self> {
        health.check()?;
        let path = disk.admin.join("credentials.json");
        let records = read_records(&path)?;
        let registry = Self {
            durability: disk.durability(),
            comb_id: disk.id(),
            key: disk.service_key()?,
            path,
            records: RwLock::new(records, health.clone()),
            health,
            broken: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fence_after_atomic: std::sync::atomic::AtomicBool::new(false),
        };
        registry.health.check()?;
        Ok(registry)
    }
    pub fn authorize(&self, credential: &Credential, claim: Option<&Claim>) -> Result<Authority> {
        self.health.check()?;
        if credential.comb_id != self.comb_id || credential.secret.len() != 64 {
            return Err(self.health.error_or(Error::forbidden()));
        }
        if credential.id.is_human() {
            if claim.is_some() {
                return Err(self.health.error_or(Error::forbidden()));
            }
            verify(
                &self.key,
                &credential.secret,
                &tag(&self.key, &human_secret(&self.key))?,
            )
            .map_err(|error| self.health.error_or(error))?;
            let authority = Authority::new(Actor::Human, true);
            self.health.check()?;
            return authority;
        }
        if self.broken.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(self.health.error_or(Error::forbidden()));
        }
        let records = self.records.read()?;
        let record = records
            .credentials
            .iter()
            .find(|r| r.id == credential.id)
            .ok_or_else(|| self.health.error_or(Error::forbidden()))?;
        verify(&self.key, &credential.secret, &record.verifier)
            .map_err(|error| self.health.error_or(error))?;
        let (actor, reviewer) = self.claimed_actor(record, claim)?;
        let authority = Authority::new(actor, reviewer);
        self.health.check()?;
        authority
    }
    fn claimed_actor(&self, record: &Record, claim: Option<&Claim>) -> Result<(Actor, bool)> {
        Ok(match (&record.grant, claim) {
            (Grant::Client { name, reviewer }, None) => (
                Actor::Client {
                    id: record.id.to_string(),
                    name: name.clone(),
                },
                *reviewer,
            ),
            (
                Grant::Bridge { reviewer_roles, .. },
                Some(Claim::Agent {
                    id,
                    role,
                    execution,
                    turn,
                }),
            ) => (
                Actor::Agent {
                    bridge: record.id.to_string(),
                    id: id.clone(),
                    role: role.clone(),
                    execution: execution.clone(),
                    turn: turn.clone(),
                },
                role.as_ref().is_some_and(|r| reviewer_roles.contains(r)),
            ),
            (Grant::Bridge { .. }, Some(Claim::Collector)) => (
                Actor::Collector {
                    bridge: record.id.to_string(),
                },
                false,
            ),
            _ => return Err(self.health.error_or(Error::forbidden())),
        })
    }
    #[cfg(test)]
    pub fn create(&self, grant: Grant) -> Result<Credential> {
        self.create_validated(grant, Grant::validate)
    }
    pub(crate) fn create_input(&self, grant: Grant) -> Result<Credential> {
        self.create_validated(grant, comb_protocol::service::input::admin::validate_grant)
    }
    fn create_validated(
        &self,
        grant: Grant,
        validate: impl FnOnce(&Grant) -> Result<()>,
    ) -> Result<Credential> {
        self.health.check()?;
        validate(&grant)?;
        let records = self.records.read()?.clone();
        if records.credentials.len() >= REGISTRY_RECORDS {
            return Err(Error::credential_count_limit());
        }
        if records
            .credentials
            .iter()
            .any(|r| r.grant.name() == grant.name())
        {
            return Err(Error::credential_name_exists());
        }
        let credential = Credential {
            comb_id: self.comb_id.clone(),
            id: CredentialId::generate(),
            secret: crate::random::secret()?,
        };
        let record = Record {
            id: credential.id.clone(),
            grant,
            verifier: tag(&self.key, &credential.secret)?,
        };
        let mut next = Records {
            credentials: records.credentials,
        };
        next.credentials.push(record);
        self.persist(next)?;
        tracing::info!(name: "comb.credential_created", success = true);
        Ok(credential)
    }
    pub fn list(&self) -> Result<Vec<Listing>> {
        let listings = self
            .records
            .read()?
            .credentials
            .iter()
            .map(|r| Listing {
                id: r.id.clone(),
                grant: r.grant.clone(),
            })
            .collect();
        self.health.check()?;
        Ok(listings)
    }
    fn persist(&self, next: Records) -> Result<()> {
        self.health.check()?;
        if self.broken.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::credential_registry_uncertain());
        }
        let bytes = crate::error::json_bytes(&next)?;
        if bytes.len() > REGISTRY_BYTES {
            return Err(Error::credential_registry_size_limit());
        }
        // Administration calls are serialized in the store lane. Authentication
        // holds no lock over file synchronization, and uncertainty fails closed.
        if let Err(e) = disk::atomic_admin(&self.path, &bytes, self.durability.as_ref()) {
            // Fence later nonhuman authorization and writes after uncertain replacement.
            self.broken.store(true, std::sync::atomic::Ordering::SeqCst);
            tracing::error!(name: "comb.credential_registry_uncertain",
                 error = &e as &(dyn std::error::Error + 'static),
                 decision = "deny_nonhuman_access");
            return Err(e);
        }
        #[cfg(test)]
        if self
            .fence_after_atomic
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.health.fence();
        }
        *self.records.write().committed()? = next;
        self.health.check().committed()
    }
    pub fn revoke(&self, id: &CredentialId) -> Result<()> {
        let records = self.records.read()?.clone();
        let next = Records {
            credentials: records
                .credentials
                .iter()
                .filter(|r| &r.id != id)
                .cloned()
                .collect(),
        };
        if next.credentials.len() == records.credentials.len() {
            return Err(Error::credential_not_found());
        }
        self.persist(next)?;
        tracing::info!(name: "comb.credential_revoked", success = true);
        Ok(())
    }
}
fn read_records(path: &Path) -> Result<Records> {
    // Absence admits no client or bridge keys; never resurrects a revoked key.
    let raw: RawRecords = if disk::exists(path)? {
        serde_json::from_slice(&disk::read(path, REGISTRY_BYTES)?)?
    } else {
        RawRecords::default()
    };
    if raw.credentials.len() > REGISTRY_RECORDS {
        return Err(Error::invalid_credential_count());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut records = Records::default();
    for record in raw.credentials {
        record.grant.validate()?;
        let id = CredentialId::issued(record.id)?;
        if !ids.insert(id.clone()) {
            return Err(Error::invalid_credential_identity());
        }
        records.credentials.push(Record {
            id,
            grant: record.grant,
            verifier: record.verifier,
        });
    }
    Ok(records)
}

/// Read-only setup inspection: no root lock, index opening or intent recovery.
pub(crate) fn inspect_bridge(root: &Path, connection: &Connection, roles: &[String]) -> Result<()> {
    let (id, key) = Disk::human_authority(root)?;
    if connection.credential.comb_id != id {
        return Err(Error::forbidden());
    }
    let records = read_records(&root.join(".comb/credentials.json"))?;
    let record = records
        .credentials
        .iter()
        .find(|r| r.id == connection.credential.id)
        .ok_or_else(Error::forbidden)?;
    let Grant::Bridge { reviewer_roles, .. } = &record.grant else {
        return Err(Error::forbidden());
    };
    let mut stored = reviewer_roles.clone();
    stored.sort();
    stored.dedup();
    if stored != roles {
        return Err(Error::forbidden());
    }
    verify(&key, &connection.credential.secret, &record.verifier)
}

/// Exact setup bridge attribution; missing is allowed only on an admitted retry.
pub(crate) fn inspect_setup_bridge(
    root: &Path,
    connection: &Connection,
    allow_missing: bool,
) -> Result<bool> {
    let (id, key) = Disk::human_authority(root)?;
    if connection.credential.comb_id != id {
        return Err(Error::forbidden());
    }
    let records = read_records(&root.join(".comb/credentials.json"))?;
    let Some(record) = records
        .credentials
        .iter()
        .find(|r| r.id == connection.credential.id)
    else {
        return if allow_missing {
            Ok(false)
        } else {
            Err(Error::forbidden())
        };
    };
    if !matches!(&record.grant, Grant::Bridge { name, .. } if name == "hive-setup") {
        return Err(Error::forbidden());
    }
    verify(&key, &connection.credential.secret, &record.verifier)?;
    Ok(true)
}

fn human_secret(key: &[u8; 32]) -> String {
    crate::page::hash(key)
}
fn tag(key: &[u8; 32], secret: &str) -> Result<[u8; 32]> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|cause| Error::forbidden().caused_by(cause))?;
    mac.update(secret.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}
fn verify(key: &[u8; 32], secret: &str, expected: &[u8; 32]) -> Result<()> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|cause| Error::forbidden().caused_by(cause))?;
    mac.update(secret.as_bytes());
    mac.verify_slice(expected)
        .map_err(|cause| Error::forbidden().caused_by(cause))
}

pub use comb_protocol::service::auth::{Claim, Connection, Credential, Grant};
/// Server-only credential publication and human authority.
pub trait ConnectionExt: Sized {
    fn write(&self, path: &Path) -> Result<()>;
    fn human(root: &Path, socket: PathBuf) -> Result<Self>;
}
impl ConnectionExt for Connection {
    fn write(&self, path: &Path) -> Result<()> {
        CredentialOutput::prepare(path)?.publish(self)
    }

    fn human(root: &Path, socket: PathBuf) -> Result<Self> {
        let (comb_id, key) = Disk::human_authority(root)?;
        Ok(Self {
            format: hive_input::compatibility::Exact,
            socket,
            credential: Credential {
                comb_id,
                id: CredentialId::human(),
                secret: human_secret(&key),
            },
        })
    }
}
