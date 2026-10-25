//! Resolve only the provider used by an operation. A Claude-only installation
//! must not require Codex (or vice versa) just to start the daemon.
use crate::{config::Provider, error::Result, process};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderSelection {
    pub codex: String,
    pub claude: String,
}

#[derive(Clone)]
pub(crate) struct Binaries {
    pub codex: OsString,
    pub claude: OsString,
}

impl Binaries {
    pub fn resolve(&self, provider: Provider) -> Result<PathBuf> {
        match provider {
            Provider::Codex => process::runtime_codex_executable(&self.codex),
            Provider::Claude => process::executable(&self.claude),
        }
    }
    pub(super) fn resolved(&self) -> ResolvedBinaries {
        let codex = self.resolve(Provider::Codex);
        let claude = self.resolve(Provider::Claude);
        let selection = ProviderSelection {
            codex: fingerprint(&self.codex, &codex),
            claude: fingerprint(&self.claude, &claude),
        };
        ResolvedBinaries {
            selection,
            codex,
            claude,
        }
    }
    pub fn selection(&self) -> ProviderSelection {
        self.resolved().selection
    }
    pub fn discovery(&self, provider: Provider) -> Result<PathBuf> {
        process::executable(match provider {
            Provider::Codex => &self.codex,
            Provider::Claude => &self.claude,
        })
    }
}

#[derive(Clone)]
pub(super) struct ResolvedBinaries {
    pub selection: ProviderSelection,
    codex: Result<PathBuf>,
    claude: Result<PathBuf>,
}
impl ResolvedBinaries {
    pub fn resolve(&self, provider: Provider) -> Result<PathBuf> {
        match provider {
            Provider::Codex => &self.codex,
            Provider::Claude => &self.claude,
        }
        .clone()
    }
}

impl Default for Binaries {
    fn default() -> Self {
        Self {
            codex: "codex".into(),
            claude: "claude".into(),
        }
    }
}

// Hash selected launch bytes, including wrapper identity. Never canonicalize them.
fn fingerprint(argument: &OsStr, resolved: &Result<PathBuf>) -> String {
    let mut hash = Sha256::new();
    match resolved {
        Ok(path) => {
            hash.update(b"resolved\0");
            hash.update(path.as_os_str().as_bytes());
        }
        Err(error) => {
            hash.update(b"unresolved\0");
            hash.update(error.code().as_bytes());
            hash.update(b"\0");
            hash.update(argument.as_bytes());
        }
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprints_preserve_non_utf8_selected_paths_and_missing_arguments() {
        use std::os::unix::ffi::OsStringExt;
        let first = OsString::from_vec(b"/private/path\xff".to_vec());
        let second = OsString::from_vec(b"/private/path\xfe".to_vec());
        assert_eq!(first.to_string_lossy(), second.to_string_lossy());
        let one = Ok(PathBuf::from(&first));
        let two = Ok(PathBuf::from(&second));
        assert_ne!(fingerprint(&first, &one), fingerprint(&second, &two));
        assert_eq!(
            fingerprint(OsStr::new("codex"), &one),
            fingerprint(&first, &one)
        );
        let failed = Err(crate::error::Error::invalid_path("Missing provider."));
        assert_ne!(fingerprint(&first, &failed), fingerprint(&second, &failed));
        assert_ne!(fingerprint(&first, &failed), fingerprint(&first, &one));
    }
}
