use super::Admin;
use crate::{Error, Result};
use hive_input::Code;
use serde::Deserialize;
use serde_json::{Map, Value};

impl Admin {
    // Serde's internally tagged unit variants ignore unknown fields even with
    // deny_unknown_fields. Enforce the same contract as the data variants.
    pub(crate) fn check_fields(self, args: &Map<String, Value>) -> Result<()> {
        if args
            .keys()
            .any(|key| key != "op" && !self.fields().contains(&key.as_str()))
        {
            return Err(self.error(Code::UnknownArgument, ""));
        }
        Ok(())
    }
    pub(crate) fn shape_error(self, args: &Map<String, Value>, cause: Error) -> Error {
        if let Err(error) = self.check_fields(args) {
            return error.caused_by(cause);
        }
        let (code, field) = self
            .shape(args)
            .unwrap_or((Code::InvalidValue, String::new()));
        self.error(code, &field).caused_by(cause)
    }
    fn shape(self, args: &Map<String, Value>) -> Option<(Code, String)> {
        for field in self.fields() {
            let Some(value) = args.get(*field) else {
                if *field == "prefix" {
                    continue;
                }
                return Some((Code::MissingArgument, (*field).into()));
            };
            if *field == "grant" {
                return grant_shape(value);
            }
            if !(value.is_string() || *field == "prefix" && value.is_null()) {
                return Some((Code::InvalidValue, (*field).into()));
            }
        }
        None
    }
}
fn grant_shape(value: &Value) -> Option<(Code, String)> {
    // Preserve all structures accepted by the native grant decoder.
    if crate::service::auth::Grant::deserialize(value).is_ok() {
        return None;
    }
    let Some(args) = value.as_object() else {
        return Some((Code::InvalidValue, "grant".into()));
    };
    let fields: &[&str] = match args.get("kind") {
        None => return Some((Code::MissingArgument, "grant/kind".into())),
        Some(Value::String(kind)) if kind == "client" => &["kind", "name", "reviewer"],
        Some(Value::String(kind)) if kind == "bridge" => &["kind", "name", "reviewer_roles"],
        _ => return Some((Code::InvalidValue, "grant/kind".into())),
    };
    if args.keys().any(|key| !fields.contains(&key.as_str())) {
        return Some((Code::UnknownArgument, "grant".into()));
    }
    match args.get("name") {
        None => return Some((Code::MissingArgument, "grant/name".into())),
        Some(Value::String(_)) => {}
        _ => return Some((Code::InvalidValue, "grant/name".into())),
    }
    if args.get("reviewer").is_some_and(|v| !v.is_boolean()) {
        return Some((Code::InvalidValue, "grant/reviewer".into()));
    }
    if let Some(roles) = args.get("reviewer_roles") {
        let field = roles
            .as_array()
            .and_then(|roles| roles.iter().position(|v| !v.is_string()))
            .filter(|index| *index < crate::service::limits::REVIEWER_ROLES)
            .map_or_else(
                || "grant/reviewer_roles".into(),
                |index| format!("grant/reviewer_roles/{index}"),
            );
        return Some((Code::InvalidValue, field));
    }
    Some((Code::InvalidValue, "grant".into()))
}
