use crate::{
    Error, Page, Result, disk, page,
    store::limits::{IMPORT_BYTES, IMPORT_PAGES},
    values::{ContentHash, Slug},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) const PATH_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub directory: PathBuf,
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    pub relative: String,
    pub source_hash: ContentHash,
    pub slug: Slug,
    pub markdown: String,
}
pub(super) struct Inputs {
    pub source: Source,
    pub pages: Vec<Input>,
}

impl Inputs {
    pub fn read(source: &Path, prefix: Option<&Slug>) -> Result<Self> {
        if !fs::symlink_metadata(source)?.is_dir() {
            return Err(Error::invalid_import_directory());
        }
        let directory = source.canonicalize()?;
        if directory.to_str().is_none_or(|s| s.len() > PATH_BYTES) {
            return Err(Error::import_path_not_utf8());
        }
        let metadata = fs::symlink_metadata(&directory)?;
        let source = Source {
            directory,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let mut files = Vec::new();
        gather(&source.directory, &source.directory, &mut files)?;
        files.sort();
        let mut pages = Vec::new();
        let mut total = 0;
        let mut slugs = BTreeSet::new();
        for (relative, path) in files {
            let relative = relative
                .to_str()
                .ok_or_else(Error::import_path_not_utf8)?
                .to_owned();
            let slug = slug(&relative, prefix)?;
            if !slugs.insert(slug.clone()) {
                return Err(Error::import_collision());
            }
            let bytes = disk::read(&path, page::PAGE_BYTES)?;
            let source_hash = ContentHash::digest(&bytes);
            let text = String::from_utf8(bytes)
                .map_err(|cause| Error::imported_page_not_utf8().caused_by(cause.utf8_error()))?;
            let markdown = markdown(&relative, text)?;
            total += markdown.len();
            if total > IMPORT_BYTES {
                return Err(Error::import_byte_limit());
            }
            pages.push(Input {
                relative,
                source_hash,
                slug,
                markdown,
            });
        }
        Ok(Self { source, pages })
    }
}
pub(super) fn slug(relative: &str, prefix: Option<&Slug>) -> Result<Slug> {
    let path = Path::new(relative);
    if relative.len() > PATH_BYTES
        || path.is_absolute()
        || path.extension().is_none_or(|s| s != "md")
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(Error::invalid_import_journal());
    }
    let stem = path.with_extension("");
    let name = stem.to_str().ok_or_else(Error::import_path_not_utf8)?;
    prefix
        .map_or_else(|| name.to_owned(), |prefix| format!("{prefix}/{name}"))
        .to_ascii_lowercase()
        .try_into()
}
fn markdown(relative: &str, text: String) -> Result<String> {
    let markdown = if text.starts_with("---\n") {
        text
    } else {
        let stem = Path::new(relative).with_extension("");
        let name = stem.to_str().ok_or_else(Error::import_path_not_utf8)?;
        let title = text
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .unwrap_or(name);
        Page::note(title, &text)?
    };
    Page::parse(&markdown)?;
    Ok(markdown)
}
fn gather(root: &Path, path: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(Error::import_symlink());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            gather(root, &entry.path(), out)?;
        }
    } else if path.extension().is_some_and(|ext| ext == "md") {
        if out.len() >= IMPORT_PAGES {
            return Err(Error::import_page_limit());
        }
        out.push((
            path.strip_prefix(root)
                .map_err(|cause| Error::import_path_escaped().caused_by(cause))?
                .to_owned(),
            path.to_owned(),
        ));
    }
    Ok(())
}
