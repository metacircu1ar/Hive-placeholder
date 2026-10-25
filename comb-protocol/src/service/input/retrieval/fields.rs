use super::{Read, metadata};
use crate::{
    Error, Result,
    values::{Slug, Timestamp},
};
use hive_input::Code;
use serde_json::{Map, Value};

pub(super) struct Fields<'a> {
    read: Read,
    prefix: &'static str,
    values: &'a Map<String, Value>,
}
impl<'a> Fields<'a> {
    pub fn new(read: Read, prefix: &'static str, values: &'a Map<String, Value>) -> Self {
        Self {
            read,
            prefix,
            values,
        }
    }
    pub fn is_tool(&self) -> bool {
        self.prefix == "/arguments"
    }
    fn error(&self, code: Code, field: &'static str) -> Error {
        let pointer = if field.is_empty() {
            self.prefix.into()
        } else {
            format!("{}/{field}", self.prefix)
        };
        metadata::error(
            code,
            &pointer,
            self.read.expected(field),
            self.read.example(self.is_tool()),
        )
    }
    pub fn known(&self, names: &[&str]) -> Result<()> {
        if self.values.keys().any(|key| !names.contains(&key.as_str())) {
            return Err(self.error(Code::UnknownArgument, ""));
        }
        Ok(())
    }
    pub fn validate<T>(&self, field: &'static str, result: Result<T>) -> Result<T> {
        result.map_err(|cause| self.error(Code::InvalidValue, field).caused_by(cause))
    }
    pub fn object(&self, field: &'static str) -> Result<&'a Map<String, Value>> {
        self.values
            .get(field)
            .ok_or_else(|| self.error(Code::MissingArgument, field))?
            .as_object()
            .ok_or_else(|| self.error(Code::InvalidValue, field))
    }
    pub fn text(&self, field: &'static str, required: bool) -> Result<Option<&'a str>> {
        match self.values.get(field) {
            None if required => Err(self.error(Code::MissingArgument, field)),
            None => Ok(None),
            Some(Value::Null) if !required => Ok(None),
            Some(Value::String(text)) => Ok(Some(text)),
            _ => Err(self.error(Code::InvalidValue, field)),
        }
    }
    pub fn slug(&self, field: &'static str) -> Result<Slug> {
        self.validate(field, self.text(field, true)?.unwrap_or_default().parse())
    }
    pub fn cursor(&self) -> Result<Option<String>> {
        self.text("after", false)?
            .map(|text| {
                self.validate("after", text.parse::<Slug>())
                    .map(|_| text.into())
            })
            .transpose()
    }
    pub fn filter(&self, field: &'static str) -> Result<Option<String>> {
        let max = match field {
            "kind" => crate::limits::KIND_BYTES,
            "tag" => crate::limits::LABEL_BYTES,
            _ => crate::values::limits::SLUG_BYTES,
        };
        self.text(field, false)?
            .map(|text| {
                self.validate(field, crate::search::filter_text(text, max))
                    .map(|()| text.into())
            })
            .transpose()
    }
    pub fn date(&self, field: &'static str) -> Result<Option<Timestamp>> {
        self.text(field, false)?
            .map(|text| self.validate(field, text.parse()))
            .transpose()
    }
    pub fn boolean(&self, field: &'static str, default: bool) -> Result<bool> {
        match self.values.get(field) {
            None => Ok(default),
            Some(Value::Bool(value)) => Ok(*value),
            _ => Err(self.error(Code::InvalidValue, field)),
        }
    }
    pub fn mode(&self) -> Result<crate::search::Mode> {
        use crate::search::Mode;
        match self.values.get("mode") {
            None => Ok(Mode::default()),
            Some(Value::String(mode)) => match mode.as_str() {
                "text" => Ok(Mode::Text),
                "vector" => Ok(Mode::Vector),
                "hybrid" => Ok(Mode::Hybrid),
                _ => Err(self.error(Code::InvalidValue, "mode")),
            },
            _ => Err(self.error(Code::InvalidValue, "mode")),
        }
    }
    pub fn number(
        &self,
        field: &'static str,
        min: usize,
        max: usize,
        default: Option<usize>,
        nullable: bool,
    ) -> Result<Option<usize>> {
        let value = match self.values.get(field) {
            None if default.is_some() || nullable => return Ok(default),
            None => return Err(self.error(Code::MissingArgument, field)),
            Some(Value::Null) if nullable => return Ok(None),
            Some(Value::Number(value)) => value,
            _ => return Err(self.error(Code::InvalidValue, field)),
        };
        let count = value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| self.error(Code::OutOfRange, field))?;
        Ok(Some(count))
    }
}
