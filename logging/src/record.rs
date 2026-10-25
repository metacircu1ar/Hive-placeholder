//! Only typed metadata crosses the logging boundary; error text never does.
use serde::Serialize;
use std::{error::Error, time::SystemTime};

const MAX_FIELDS: usize = 32;
const MAX_ID_BYTES: usize = 128;
const MAX_CAUSES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}
impl From<tracing::Level> for Level {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::TRACE => Self::Trace,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::INFO => Self::Info,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::ERROR => Self::Error,
        }
    }
}
#[cfg(test)]
impl Level {
    pub fn tracing(self) -> tracing::Level {
        match self {
            Self::Trace => tracing::Level::TRACE,
            Self::Debug => tracing::Level::DEBUG,
            Self::Info => tracing::Level::INFO,
            Self::Warn => tracing::Level::WARN,
            Self::Error => tracing::Level::ERROR,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invalid {
    Name,
    Identity,
    Duplicate,
    Capacity,
    Clock,
    Encoding,
    Oversized,
    NumericRange,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Value {
    Count(u64),
    Flag(bool),
    Code(String),
    Signed(i64),
    Float(f64),
    Context(Vec<SpanContext>),
    Identity(String),
    Causes(Causes),
    Location {
        file: String,
        line: u32,
        column: u32,
    },
}

#[derive(Default, Serialize)]
#[serde(transparent)]
pub struct Fields(std::collections::BTreeMap<&'static str, Value>);
impl Fields {
    pub fn count(&mut self, key: &'static str, value: u64) -> Result<(), Invalid> {
        self.insert(key, Value::Count(value))
    }
    pub fn flag(&mut self, key: &'static str, value: bool) -> Result<(), Invalid> {
        self.insert(key, Value::Flag(value))
    }
    pub fn code(&mut self, key: &'static str, value: &str) -> Result<(), Invalid> {
        if !name(value) {
            return Err(Invalid::Name);
        }
        self.insert(key, Value::Code(value.to_owned()))
    }
    pub fn identity(&mut self, key: &'static str, value: &str) -> Result<(), Invalid> {
        if value.is_empty()
            || value.len() > MAX_ID_BYTES
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        {
            return Err(Invalid::Identity);
        }
        self.insert(key, Value::Identity(value.to_owned()))
    }
    pub fn cause(&mut self, error: &(dyn Error + 'static)) -> Result<(), Invalid> {
        self.insert("cause", Value::Causes(Causes::classify(error)))
    }
    pub(crate) fn location(&mut self, path: &str, line: u32, column: u32) -> Result<(), Invalid> {
        let file = path.rsplit(['/', '\\']).next().unwrap_or("unknown");
        let file = if file.len() <= 128
            && file
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            file
        } else {
            "unknown"
        };
        self.insert(
            "location",
            Value::Location {
                file: file.to_owned(),
                line,
                column,
            },
        )
    }
    pub(crate) fn signed(&mut self, key: &'static str, value: i64) -> Result<(), Invalid> {
        self.insert(key, Value::Signed(value))
    }
    pub(crate) fn float(&mut self, key: &'static str, value: f64) -> Result<(), Invalid> {
        if !value.is_finite() {
            return Err(Invalid::NumericRange);
        }
        self.insert(key, Value::Float(value))
    }
    pub(crate) fn error(
        &mut self,
        key: &'static str,
        error: &(dyn Error + 'static),
    ) -> Result<(), Invalid> {
        self.insert(key, Value::Causes(Causes::classify(error)))
    }
    pub(crate) fn provenance(&mut self, key: &'static str, value: &str) -> Result<(), Invalid> {
        if !crate::policy::target_name(value) {
            return Err(Invalid::Name);
        }
        self.insert(key, Value::Code(value.to_owned()))
    }
    pub(crate) fn context(
        &mut self,
        spans: Vec<SpanContext>,
        truncated: bool,
    ) -> Result<(), Invalid> {
        self.insert("spans", Value::Context(spans))?;
        self.flag("spans_truncated", truncated)
    }
    fn insert(&mut self, key: &'static str, value: Value) -> Result<(), Invalid> {
        if !name(key) || matches!(key, "t" | "level" | "event") {
            return Err(Invalid::Name);
        }
        if self.0.len() >= MAX_FIELDS {
            return Err(Invalid::Capacity);
        }
        if self.0.contains_key(key) {
            return Err(Invalid::Duplicate);
        }
        self.0.insert(key, value);
        Ok(())
    }
}

pub(crate) fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
}

#[derive(Serialize)]
pub(crate) struct SpanContext {
    pub name: &'static str,
    pub id: u64,
}

#[derive(Debug, Serialize)]
pub struct Cause {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    io_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    errno: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<usize>,
}
#[derive(Debug, Serialize)]
pub struct Causes {
    items: Vec<Cause>,
    truncated: bool,
}
impl Causes {
    pub fn classify(error: &(dyn Error + 'static)) -> Self {
        let mut current = Some(error);
        let mut items = Vec::new();
        for _ in 0..MAX_CAUSES {
            let Some(error) = current else {
                break;
            };
            let mut item = Cause {
                kind: "other",
                io_kind: None,
                errno: None,
                line: None,
                column: None,
            };
            if let Some(error) = error.downcast_ref::<std::io::Error>() {
                item.kind = "io";
                // Debug only the OS ErrorKind enum, never an Error value.
                item.io_kind = Some(format!("{:?}", error.kind()));
                item.errno = error.raw_os_error();
            } else if let Some(error) = error.downcast_ref::<serde_json::Error>() {
                item.kind = match error.classify() {
                    serde_json::error::Category::Io => "json_io",
                    serde_json::error::Category::Syntax => "json_syntax",
                    serde_json::error::Category::Data => "json_data",
                    serde_json::error::Category::Eof => "json_eof",
                };
                item.line = Some(error.line());
                item.column = Some(error.column());
            }
            items.push(item);
            current = error.source();
        }
        Self {
            items,
            truncated: current.is_some(),
        }
    }
}

#[derive(Serialize)]
struct Record {
    t: u64,
    level: Level,
    event: &'static str,
    #[serde(flatten)]
    fields: Fields,
}
pub(crate) fn encode(
    level: Level,
    event: &'static str,
    fields: Fields,
    limit: usize,
) -> Result<Vec<u8>, Invalid> {
    if !name(event) {
        return Err(Invalid::Name);
    }
    let elapsed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| Invalid::Clock)?;
    let t = u64::try_from(elapsed.as_millis()).map_err(|_| Invalid::Clock)?;
    let mut bytes = serde_json::to_vec(&Record {
        t,
        level,
        event,
        fields,
    })
    .map_err(|_| Invalid::Encoding)?;
    bytes.push(b'\n');
    if bytes.len() > limit {
        return Err(Invalid::Oversized);
    }
    Ok(bytes)
}
