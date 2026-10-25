//! Strict current manifests, bounded files and native platform checks.
use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
    process::Command,
};

fn validate_product_version(value: &str) -> Result<()> {
    if value.len() > 128 {
        return Err("Oversized product version".into());
    }
    semver::Version::parse(value)?;
    Ok(())
}

/// Components retain product names; maintenance invokes their CLI executable.
pub(crate) fn maintenance_binary(component: &str) -> &str {
    match component {
        "hive" => "hivectl",
        other => other,
    }
}

pub const MAX_BYTES: u64 = 512 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub sha256: String,
    pub bytes: u64,
    pub mode: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: crate::compatibility::Format,
    pub product_version: String,
    pub part: String,
    pub target: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub required_glibc: Option<String>,
    pub files: BTreeMap<String, File>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub format: crate::compatibility::Format,
    pub target: String,
    pub parts: BTreeMap<String, Manifest>,
    pub installer: File,
    pub installer_notices: File,
    pub retired: BTreeMap<String, std::path::PathBuf>,
}

pub fn native_target() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        arch => return Err(format!("Unsupported CPU: {arch}").into()),
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        os => {
            return Err(format!(
                "Unsupported operating system: {os}; only macOS and GNU/Linux are supported"
            )
            .into());
        }
    };
    Ok(format!("{arch}-{os}"))
}
pub fn regular(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(format!("Expected owned regular file: {}", path.display()).into());
    }
    Ok(metadata)
}
pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_file(path, limit).map_err(|cause| {
        asset(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            cause,
        )
    })
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if regular(path)?.len() > limit {
        return Err("Oversized release metadata".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("Oversized release metadata".into());
    }
    Ok(bytes)
}
pub(crate) fn asset(
    name: String,
    cause: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(AssetError { name, cause })
}
#[derive(Debug)]
struct AssetError {
    name: String,
    cause: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Release asset {}: {}", self.name, self.cause)
    }
}
impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}
pub fn hash(path: &Path) -> Result<String> {
    let mut input = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn safe_path(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && name
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
pub fn version(value: &str) -> Result<Vec<u32>> {
    let pieces: Vec<u32> = value
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    if !(2..=3).contains(&pieces.len()) {
        return Err("Invalid GLIBC requirement".into());
    }
    Ok(pieces)
}
impl Manifest {
    pub fn component(&self) -> Result<&'static str> {
        match self.part.as_str() {
            "hive" | "hive-core" => Ok("hive"),
            "comb" => Ok("comb"),
            _ => Err("Unknown release part".into()),
        }
    }
    pub fn binaries(&self) -> Result<&'static [&'static str]> {
        Ok(if self.component()? == "hive" {
            &["hivectl", "hive-daemon"]
        } else {
            &["comb"]
        })
    }
    pub fn validate(&self, target: &str) -> Result<()> {
        validate_product_version(&self.product_version)?;
        if self.target != target || self.files.len() > 128 {
            return Err("Wrong release target or file count".into());
        }
        let prefix = format!("share/{}/", self.component()?);
        let binaries: BTreeSet<String> = self
            .binaries()?
            .iter()
            .map(|n| format!("bin/{n}"))
            .collect();
        for path in binaries.iter().cloned().chain([
            format!("{prefix}THIRD_PARTY_LICENSES"),
            format!("{prefix}build.json"),
        ]) {
            if !self.files.contains_key(&path) {
                return Err(format!("Required payload missing: {path}").into());
            }
        }
        let mut bytes = 0_u64;
        for (path, file) in &self.files {
            if !safe_path(path)
                || (!binaries.contains(path) && !path.starts_with(&prefix))
                || file.mode
                    != if binaries.contains(path) {
                        0o755
                    } else {
                        0o644
                    }
                || file.sha256.len() != 64
                || !file
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err(format!("Invalid release payload: {path}").into());
            }
            bytes = bytes
                .checked_add(file.bytes)
                .ok_or("Payload size overflow")?;
        }
        if bytes > MAX_BYTES {
            return Err("Release payload exceeds limit".into());
        }
        if target.contains("linux") {
            let needed = version(
                self.required_glibc
                    .as_deref()
                    .ok_or("Missing GLIBC requirement")?,
            )?;
            let host = Command::new("getconf").arg("GNU_LIBC_VERSION").output()?;
            let text = String::from_utf8(host.stdout)?;
            let current = text
                .trim()
                .strip_prefix("glibc ")
                .ok_or("GNU/Linux with glibc required")?;
            if !host.status.success()
                || version(current)? < needed
                || version(current)? < vec![2, 35]
            {
                return Err(format!(
                    "glibc >= 2.35 and >= {} required; found {current}",
                    self.required_glibc.as_deref().unwrap_or("")
                )
                .into());
            }
        } else if self.required_glibc.is_some() {
            return Err("GLIBC requirement on a non-Linux artifact".into());
        }
        Ok(())
    }
    pub fn check_files(&self, root: &Path) -> Result<()> {
        for (name, expected) in &self.files {
            let path = root.join(name);
            // Check each ancestor too: an edited installation cannot redirect hard links.
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if directory == root {
                    break;
                }
                if !fs::symlink_metadata(directory)?.is_dir() {
                    return Err("Payload parent is not a real directory".into());
                }
                parent = directory.parent();
            }
            let metadata = regular(&path)?;
            if metadata.len() != expected.bytes
                || metadata.permissions().mode() & 0o7777 != expected.mode
                || hash(&path)? != expected.sha256
            {
                return Err(format!("Owned payload was modified: {}", path.display()).into());
            }
        }
        Ok(())
    }
    pub fn check_versions(&self, root: &Path) -> Result<()> {
        let suffix = match self.part.as_str() {
            "hive" => "(comb)",
            "hive-core" => "(core)",
            "comb" => "(http)",
            _ => return Err("Unknown part".into()),
        };
        for name in self.binaries()? {
            let path = root.join("bin").join(name);
            // Running the actual payload makes the platform loader check every
            // versioned symbol against the exact host libraries it resolves.
            let result = Command::new(&path)
                .arg("--version")
                .output()
                .map_err(|error| {
                    crate::publication::context(
                        "Cannot execute payload. If temporary storage is noexec, set TMPDIR to a \
                    private directory on an executable filesystem",
                        error.into(),
                    )
                })?;
            let text = String::from_utf8_lossy(&result.stdout);
            if !result.status.success() || !text.trim().ends_with(suffix) {
                return Err(format!(
                    "Installed variant/loader check failed: {name}: {}; required GLIBC={:?}; \
                    stdout: {}; stderr: {}",
                    result.status,
                    self.required_glibc,
                    text.trim(),
                    String::from_utf8_lossy(&result.stderr).trim()
                )
                .into());
            }
        }
        Ok(())
    }
}
