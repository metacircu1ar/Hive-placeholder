//! Stage all sources before publishing, streaming one descriptor at a time.
use super::{
    Attachment, FILE_BYTES, LiveReason, MAX_FILES, MESSAGE_BYTES, Result, Source, files, invalid,
    media, valid_name,
};
use crate::{error::Error, state};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const BUFFER_BYTES: usize = 64 * 1024;
pub(in crate::runtime) struct Capture {
    root: PathBuf,
    private: PathBuf,
    cached: HashMap<PathBuf, Cached>,
    staged: Vec<Staged>,
}
struct Cached {
    item: Attachment,
    path: PathBuf,
    observed: super::Identity,
}
struct Staged {
    key: String,
    name: String,
    file: tempfile::TempPath,
}
impl Capture {
    pub(in crate::runtime) fn new(root: PathBuf, private: PathBuf) -> Result<Self> {
        state::directory(&root)?;
        let root = root.canonicalize().map_err(Error::state_io_from)?;
        let private = private.canonicalize().map_err(Error::state_io_from)?;
        Ok(Self {
            root,
            private,
            cached: HashMap::new(),
            staged: Vec::new(),
        })
    }
    pub(in crate::runtime) fn message(&mut self, paths: &[PathBuf]) -> Result<Vec<Attachment>> {
        if paths.len() > MAX_FILES {
            return Err(Error::attachment_invalid(
                "A message accepts at most 10 attachments.",
            ));
        }
        let mut total = 0;
        let mut exhausted = false;
        paths
            .iter()
            .map(|path| {
                let mut item = self.capture(path, total, exhausted)?;
                if item.bytes > FILE_BYTES {
                    self.make_live(&mut item, path, LiveReason::FileSizeLimit)?;
                } else if exhausted || total + item.bytes > MESSAGE_BYTES {
                    exhausted = true;
                    self.make_live(&mut item, path, LiveReason::MessageSizeLimit)?;
                } else {
                    total += item.bytes;
                }
                Ok(item)
            })
            .collect()
    }
    fn make_live(&self, item: &mut Attachment, path: &Path, reason: LiveReason) -> Result<()> {
        if let Source::LivePath { reason: stored, .. } = &mut item.source {
            *stored = reason;
            return Ok(());
        }
        let cached = self
            .cached
            .get(path)
            .ok_or_else(|| invalid(path, "captured source identity is missing."))?;
        item.source = Source::LivePath {
            path: cached.path.clone(),
            observed: cached.observed.clone(),
            reason,
        };
        Ok(())
    }
    fn capture(&mut self, path: &Path, used: u64, exhausted: bool) -> Result<Attachment> {
        if let Some(cached) = self.cached.get(path) {
            return Ok(cached.item.clone());
        }
        let filename = filename(path)?;
        let mut source = files::SourceFile::open(path, &self.private)?;
        if source.path.starts_with(&self.root) {
            return Err(invalid(
                path,
                "private attachment snapshots cannot be forwarded directly.",
            ));
        }
        let captured_path = source.path.clone();
        let observed = source.observed.clone();
        let size = source.observed.bytes;
        let reason = live_reason(size, used, exhausted);
        let (source_kind, prefix) = if let Some(reason) = reason {
            let mut prefix = vec![0; BUFFER_BYTES.min(usize::try_from(size).unwrap_or(usize::MAX))];
            source
                .file
                .read_exact(&mut prefix)
                .map_err(|e| invalid(path, "cannot read source.").with_source(e))?;
            source.unchanged()?;
            (
                Source::LivePath {
                    path: source.path,
                    observed: source.observed,
                    reason,
                },
                prefix,
            )
        } else {
            let staged = tempfile::Builder::new()
                .prefix(".capture-")
                .tempfile_in(&self.root)
                .map_err(Error::state_io_from)?;
            let (staged, digest, prefix) = copy(&mut source, staged)?;
            let mut hash = Sha256::new();
            hash.update(digest);
            hash.update(filename.as_bytes());
            let key = format!("{:x}", hash.finalize());
            let target = self.root.join(&key).join(&filename);
            self.staged.push(Staged {
                key: key.clone(),
                name: filename.clone(),
                file: staged.into_temp_path(),
            });
            (
                Source::Snapshot {
                    key,
                    sha256: format!("{digest:x}"),
                    path: target,
                },
                prefix,
            )
        };
        let (media_type, dimensions) = media::inspect(&prefix, size > prefix.len() as u64);
        let item = Attachment {
            delivery: super::Delivery::Pending,
            filename,
            bytes: size,
            media_type: media_type.into(),
            dimensions,
            source: source_kind,
        };
        // Live choices depend on each message's budget; do not reuse them as snapshots.
        if item.key().is_some() {
            self.cached.insert(
                path.to_owned(),
                Cached {
                    item: item.clone(),
                    path: captured_path,
                    observed,
                },
            );
        }
        Ok(item)
    }
    pub(in crate::runtime) fn candidates(&self) -> Vec<super::Snapshot> {
        self.staged
            .iter()
            .map(|s| super::Snapshot {
                key: s.key.clone(),
                filename: s.name.clone(),
            })
            .collect()
    }
    pub(in crate::runtime) fn publish(self) -> Result<()> {
        for Staged { key, name, file } in self.staged {
            files::publish(&self.root, &key, &name, file)?;
        }
        Ok(())
    }
}
fn live_reason(size: u64, used: u64, exhausted: bool) -> Option<LiveReason> {
    if size > FILE_BYTES {
        Some(LiveReason::FileSizeLimit)
    } else if exhausted || used + size > MESSAGE_BYTES {
        Some(LiveReason::MessageSizeLimit)
    } else {
        None
    }
}
fn copy(
    source: &mut files::SourceFile,
    mut target: tempfile::NamedTempFile,
) -> Result<(
    tempfile::NamedTempFile,
    sha2::digest::Output<Sha256>,
    Vec<u8>,
)> {
    let mut hash = Sha256::new();
    let mut prefix = Vec::new();
    let mut count = 0_u64;
    let mut buffer = vec![0; BUFFER_BYTES];
    loop {
        let left = usize::try_from((FILE_BYTES + 1 - count).min(BUFFER_BYTES as u64))
            .unwrap_or(BUFFER_BYTES);
        let n = source
            .file
            .read(&mut buffer[..left])
            .map_err(|e| invalid(&source.path, "cannot read source.").with_source(e))?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > source.observed.bytes || count > FILE_BYTES {
            return Err(invalid(&source.path, "source grew during capture."));
        }
        if prefix.len() < BUFFER_BYTES {
            prefix.extend_from_slice(&buffer[..n.min(BUFFER_BYTES - prefix.len())]);
        }
        hash.update(&buffer[..n]);
        target
            .write_all(&buffer[..n])
            .map_err(Error::state_io_from)?;
    }
    if count != source.observed.bytes {
        return Err(invalid(&source.path, "source changed during capture."));
    }
    source.unchanged()?;
    target.as_file().sync_all().map_err(Error::state_io_from)?;
    Ok((target, hash.finalize(), prefix))
}

fn filename(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| valid_name(n))
        .ok_or_else(|| invalid(path, "filename must be valid UTF-8 and at most 255 bytes."))?
        .to_owned();
    Ok(name)
}
