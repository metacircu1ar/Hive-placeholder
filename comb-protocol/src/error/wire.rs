//! A strict object envelope; input metadata is either complete or absent.
#[cfg(test)]
mod tests;
use super::{Error, ErrorCode};
use hive_input::{Code, InputError};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeMap};

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("code", &self.code)?;
        if let Some(code) = &self.raw_code {
            map.serialize_entry("raw_code", code)?;
        }
        map.serialize_entry("message", &self.message)?;
        map.serialize_entry("file_committed", &self.file_committed)?;
        if let Some(id) = &self.request_id {
            map.serialize_entry("request_id", id)?;
        }
        if let Some(progress) = &self.import_progress {
            map.serialize_entry("import_progress", progress)?;
        }
        if let Some(input) = &self.input {
            map.serialize_entry("field", input.field())?;
            map.serialize_entry("expected", input.expected())?;
            map.serialize_entry("examples", input.examples())?;
        }
        map.end()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    code: String,
    #[serde(default, deserialize_with = "present")]
    raw_code: Option<String>,
    message: String,
    file_committed: bool,
    request_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    import_progress: Option<Box<crate::import::ImportProgress>>,
    #[serde(default, deserialize_with = "present")]
    field: Option<String>,
    #[serde(default, deserialize_with = "present")]
    expected: Option<String>,
    #[serde(default, deserialize_with = "present")]
    examples: Option<Vec<String>>,
}

fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl Envelope {
    fn error<E: de::Error>(self) -> Result<Error, E> {
        let (code, raw_code) = match external_code::<E>(&self.code)? {
            Some(code) => (code, None),
            // The external wire enum is open. Keep strict ErrorCode readers for
            // internal records, while preserving acceptance metadata from a newer peer.
            None => (ErrorCode::UnknownError, Some(self.code)),
        };
        let raw_code = match self.raw_code {
            Some(raw) if code == ErrorCode::UnknownError && raw_code.is_none() => {
                if external_code::<E>(&raw)?.is_some() {
                    return Err(E::custom("Raw error code must be unknown"));
                }
                Some(raw)
            }
            Some(_) => return Err(E::custom("Raw error code requires unknown_error")),
            None => raw_code,
        };
        if let Some(id) = &self.request_id {
            crate::values::RequestId::try_from(id.clone()).map_err(E::custom)?;
        }
        let mut error = match (self.field, self.expected, self.examples) {
            (None, None, None) if !matches!(code, ErrorCode::Input(_)) => {
                Error::new(code, self.message)
            }
            (Some(field), Some(expected), Some(examples)) => {
                let code = Code::deserialize(de::value::StrDeserializer::<E>::new(code.as_str()))?;
                InputError::new(code, self.message, field, expected, examples)
                    .map(Error::from)
                    .map_err(E::custom)?
            }
            _ => return Err(E::custom("input-error metadata must be complete")),
        };
        error.file_committed = self.file_committed;
        error.request_id = self.request_id;
        error.import_progress = self.import_progress;
        error.raw_code = raw_code;
        Ok(error)
    }
}

fn external_code<E: de::Error>(code: &str) -> Result<Option<ErrorCode>, E> {
    if code.is_empty()
        || code.len() > 128
        || !code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(E::custom("Invalid error code"));
    }
    match ErrorCode::deserialize(de::value::StrDeserializer::<de::value::Error>::new(code)) {
        Ok(code) => Ok(Some(code)),
        // Unknown external variants are retained, not treated as refusals or retried.
        Err(_) => Ok(None),
    }
}

impl<'de> Deserialize<'de> for Error {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> de::Visitor<'de> for Object {
            type Value = Error;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an Comb error object")
            }
            fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Error, M::Error> {
                Envelope::deserialize(de::value::MapAccessDeserializer::new(map))?.error()
            }
        }
        d.deserialize_map(Object)
    }
}
