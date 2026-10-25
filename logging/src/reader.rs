//! Read-only diagnostic snapshots. Cursors detect retention loss; they are not replay tokens.
mod files;
mod view;
use files::{Identity, Segment};
use serde_json::Value;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
pub use view::View;

/// Upper bound on bytes inspected by one tail or follow page.
pub const PAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_TAIL: usize = 2000;
const RECORD_BYTES: usize = 8192;

#[derive(Clone, Debug)]
pub struct Cursor {
    identity: Identity,
    offset: u64,
}

#[derive(Default)]
pub struct Filter {
    pub level: Option<tracing::Level>,
    pub event: Option<String>,
    pub agent_id: Option<String>,
}
impl Filter {
    fn accepts(&self, value: &Value) -> bool {
        self.event
            .as_deref()
            .is_none_or(|event| value["event"] == event)
            && self
                .agent_id
                .as_deref()
                .is_none_or(|id| value["agent_id"] == id)
            && self.level.is_none_or(|minimum| {
                value["level"]
                    .as_str()
                    .and_then(|s| s.parse::<tracing::Level>().ok())
                    .is_some_and(|level| level <= minimum)
            })
    }
}

#[derive(Default)]
pub struct Page {
    pub records: VecDeque<Value>,
    pub cursor: Option<Cursor>,
    pub warnings: Warnings,
    pub more: bool,
}
#[derive(Default)]
pub struct Warnings {
    pub pruned: bool,
    pub incomplete: bool,
    pub corrupt: u64,
    pub limited: bool,
}

pub struct Reader {
    directory: File,
    path: PathBuf,
    name: String,
}
impl Reader {
    /// No directories, locks or state files are created by a reader.
    pub fn open(path: &Path) -> io::Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| invalid("Log filename is invalid"))?;
        let parent = path
            .parent()
            .ok_or_else(|| invalid("Log directory is missing"))?;
        Ok(Self {
            directory: files::directory(parent)?,
            path: parent.into(),
            name: name.into(),
        })
    }
    pub fn tail(&self, count: usize, filter: &Filter) -> io::Result<Page> {
        if !(1..=MAX_TAIL).contains(&count) {
            return Err(invalid("Log tail must be 1..2000 records"));
        }
        let mut files = self.files()?;
        let mut remaining = PAGE_BYTES;
        let mut page = Page::default();
        for segment in files.iter_mut().rev() {
            let start = segment.length.saturating_sub(remaining);
            let mut piece = read(segment, start, remaining, filter, count, true)?;
            if page.cursor.is_none() {
                page.cursor = piece.cursor.take();
            }
            remaining = remaining.saturating_sub(segment.length - start);
            page.warnings.incomplete |= piece.warnings.incomplete;
            page.warnings.corrupt += piece.warnings.corrupt;
            while let Some(record) = piece.records.pop_back() {
                page.records.push_front(record);
                if page.records.len() == count {
                    break;
                }
            }
            if page.records.len() == count {
                break;
            }
            if remaining == 0 {
                page.warnings.limited = true;
                break;
            }
        }
        Ok(page)
    }
    pub fn after(&self, cursor: Option<&Cursor>, filter: &Filter) -> io::Result<Page> {
        let mut files = self.files()?;
        let position = cursor.and_then(|c| files.iter().position(|f| f.identity == c.identity));
        let mut page = Page {
            warnings: Warnings {
                pruned: cursor.is_some() && position.is_none(),
                ..Warnings::default()
            },
            cursor: cursor.filter(|_| position.is_some()).cloned(),
            ..Page::default()
        };
        let mut remaining = PAGE_BYTES;
        for segment in files.iter_mut().skip(position.unwrap_or(0)) {
            let start = cursor
                .filter(|c| c.identity == segment.identity)
                .map_or(0, |c| c.offset);
            if start > segment.length {
                page.warnings.pruned = true;
            }
            let start = if start > segment.length { 0 } else { start };
            let piece = read(
                segment,
                start,
                remaining,
                filter,
                MAX_TAIL - page.records.len(),
                false,
            )?;
            let consumed = (segment.length - start).min(remaining);
            remaining -= consumed;
            page.warnings.incomplete |= piece.warnings.incomplete;
            page.warnings.corrupt += piece.warnings.corrupt;
            page.records.extend(piece.records);
            page.cursor = piece.cursor;
            if piece.more
                || page.records.len() == MAX_TAIL
                || consumed < segment.length - start
                || remaining == 0
            {
                page.more = true;
                break;
            }
        }
        Ok(page)
    }
    fn files(&self) -> io::Result<Vec<Segment>> {
        // Following a removed/replaced log directory would silently observe the wrong stream.
        if files::identity(&files::directory(&self.path)?)? != files::identity(&self.directory)? {
            return Err(invalid("Log directory changed while reading"));
        }
        // A rotation can rename/unlink an entry between enumeration and open.
        for _ in 0..3 {
            match files::segments(&self.directory, &self.name) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::WouldBlock
                    ) => {}
                result => return result,
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Log rotation is in progress",
        ))
    }
}

fn read(
    segment: &mut Segment,
    start: u64,
    budget: u64,
    filter: &Filter,
    limit: usize,
    tail: bool,
) -> io::Result<Page> {
    let end = segment.length.min(start.saturating_add(budget));
    segment.file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut segment.file)
        .take(end - start)
        .read_to_end(&mut bytes)?;
    let mut begin = 0;
    // A tail may begin within a record; follow offsets always follow a newline.
    if start > 0 {
        segment.file.seek(SeekFrom::Start(start - 1))?;
        let mut previous = [0];
        segment.file.read_exact(&mut previous)?;
        if previous != *b"\n" {
            begin = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |p| p + 1);
        }
    }
    let complete = bytes[begin..]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |p| p + 1);
    let partial = bytes.len() - begin - complete;
    let mut page = Page {
        warnings: Warnings {
            incomplete: end == segment.length && partial > 0,
            ..Warnings::default()
        },
        cursor: Some(Cursor {
            identity: segment.identity,
            offset: start + begin as u64 + complete as u64,
        }),
        ..Page::default()
    };
    let mut processed = 0;
    for line in bytes[begin..begin + complete].split_inclusive(|b| *b == b'\n') {
        processed += line.len();
        let value = if line.len() > RECORD_BYTES {
            None
        } else {
            serde_json::from_slice::<Value>(line)
                .ok()
                .filter(|v| v.is_object() && v["event"].is_string())
        };
        match value {
            Some(value) if filter.accepts(&value) => {
                page.records.push_back(value);
                if tail && page.records.len() > limit {
                    page.records.pop_front();
                }
                if !tail && page.records.len() == limit {
                    if let Some(cursor) = &mut page.cursor {
                        cursor.offset = start + begin as u64 + processed as u64;
                    }
                    page.more = processed < complete || end < segment.length;
                    if page.more {
                        return Ok(page);
                    }
                }
            }
            Some(_) => (),
            None => page.warnings.corrupt += 1,
        }
    }
    // Never spin at the same offset on an oversized unterminated record.
    if partial > RECORD_BYTES {
        page.warnings.corrupt += 1;
        if let Some(cursor) = &mut page.cursor {
            cursor.offset = end;
        }
    }
    Ok(page)
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
