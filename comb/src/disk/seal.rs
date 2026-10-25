//! Comb identity, page seals and the separate service authority key.
use super::limits::IDENTITY_BYTES;
use super::{Disk, System, dir, read};
use crate::{
    Error, Page, Result,
    values::{CombId, ContentHash, Slug},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{fs, os::unix::fs::MetadataExt, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub(super) id: CombId,
    pub(super) key: [u8; 32],
}

impl Disk {
    /// A separate service key; the seal key itself never leaves this module.
    pub fn service_key(&self) -> Result<[u8; 32]> {
        service_key(&self.identity)
    }
    pub fn human_authority(root: &Path) -> Result<(CombId, [u8; 32])> {
        let admin = root.join(".comb");
        dir(&admin, false, &System)?;
        if fs::metadata(&admin)?.mode() & 0o077 != 0 {
            return Err(Error::unsafe_admin_directory());
        }
        super::initialization::check_format(&admin)?;
        let identity: Identity =
            serde_json::from_slice(&read(&admin.join("identity.json"), IDENTITY_BYTES)?)?;
        Ok((identity.id.clone(), service_key(&identity)?))
    }
    fn mac(&self, slug: &Slug, page: &Page) -> Result<Hmac<Sha256>> {
        let unsigned = page.unsigned();
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.identity.key)
            .map_err(|cause| Error::invalid_authentication_key().caused_by(cause))?;
        mac.update(&crate::error::json_bytes(&(
            &self.identity.id,
            slug,
            unsigned,
        ))?);
        Ok(mac)
    }
    pub fn seal(&self, slug: &Slug, page: &mut Page) -> Result<()> {
        let tag = self.mac(slug, page)?.finalize().into_bytes();
        page.install_seal(format!("{tag:x}"));
        Ok(())
    }
    pub(super) fn valid_seal(&self, slug: &Slug, page: &Page) -> Result<bool> {
        let Some(seal) = page.metadata.owned().seal() else {
            return Ok(false);
        };
        if seal.len() != 64 || !seal.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(false);
        }
        let bytes = (0..64)
            .step_by(2)
            .map(|i| u8::from_str_radix(&seal[i..i + 2], 16))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|cause| Error::invalid_seal().caused_by(cause))?;
        Ok(self.mac(slug, page)?.verify_slice(&bytes).is_ok())
    }
    pub fn trusted(&self, slug: &Slug, page: &Page, raw: &str) -> Result<bool> {
        Ok(self.head(slug)?.as_ref() == Some(&ContentHash::digest(raw))
            && self.valid_seal(slug, page)?)
    }
}

pub(super) fn service_key(identity: &Identity) -> Result<[u8; 32]> {
    let mut mac = Hmac::<Sha256>::new_from_slice(&identity.key)
        .map_err(|cause| Error::invalid_authority_key().caused_by(cause))?;
    mac.update(b"comb-service-authority");
    Ok(mac.finalize().into_bytes().into())
}
