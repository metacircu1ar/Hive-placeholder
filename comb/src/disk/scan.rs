//! Lazy native-order page traversal, owning each open directory frame.
use super::{System, dir, filesystem::same_device, read};
use crate::{Error, Result, page, values::Slug};
use std::{ffi::OsString, fs, path::PathBuf};

fn entry_name(name: OsString) -> Result<String> {
    name.into_string().map_err(|cause| {
        Error::page_path_not_utf8().caused_by(crate::error::Cause::non_utf8_path(cause))
    })
}

struct Frame {
    entries: fs::ReadDir,
    prefix: String,
}

pub(crate) struct Pages {
    root: Option<PathBuf>,
    device: u64,
    stack: Vec<Frame>,
}

impl Pages {
    pub(super) fn new(root: PathBuf, device: u64) -> Self {
        Self {
            root: Some(root),
            device,
            stack: Vec::new(),
        }
    }

    fn advance(&mut self) -> Result<Option<(Slug, Result<String>)>> {
        if let Some(root) = self.root.take() {
            self.stack.push(Frame {
                entries: fs::read_dir(root)?,
                prefix: String::new(),
            });
        }
        loop {
            let Some(frame) = self.stack.last_mut() else {
                return Ok(None);
            };
            let Some(entry) = frame.entries.next() else {
                self.stack.pop();
                continue;
            };
            let entry = entry?;
            let name = entry_name(entry.file_name())?;
            if name.starts_with('.') {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(Error::scan_symlink());
            }
            same_device(&entry.path(), self.device)?;
            if kind.is_dir() {
                dir(&entry.path(), false, &System)?;
                let prefix = format!("{}{name}/", frame.prefix);
                let entries = fs::read_dir(entry.path())?;
                self.stack.push(Frame { entries, prefix });
            } else if let Some(stem) = name.strip_suffix(".md") {
                let slug = format!("{}{stem}", frame.prefix);
                let slug = Slug::try_from(slug)?;
                let content = match read(&entry.path(), page::PAGE_BYTES) {
                    Ok(bytes) => String::from_utf8(bytes)
                        .map_err(|cause| Error::page_not_utf8().caused_by(cause.utf8_error())),
                    Err(error) if error.code == crate::ErrorCode::SizeLimit => Err(error),
                    Err(error) => return Err(error),
                };
                return Ok(Some((slug, content)));
            }
        }
    }
}

impl Iterator for Pages {
    // Outer errors are unsafe paths or operational failures and stop traversal.
    // Inner errors are bounded malformed page content and can be quarantined.
    type Item = Result<(Slug, Result<String>)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.advance() {
            Ok(page) => page.map(Ok),
            Err(error) => {
                self.root = None;
                self.stack.clear();
                Some(Err(error))
            }
        }
    }
}
impl std::iter::FusedIterator for Pages {}

#[cfg(test)]
mod tests;
