//! Diagnose only native decoder failures; never replace its acceptance contract.
use super::Write;
use crate::Error;
use hive_input::Code;
use serde::Deserialize;
use serde_json::{Map, Value};

impl Write {
    pub fn shape_error(self, args: &Map<String, Value>, tool: bool, cause: Error) -> Error {
        let (code, field) = self.shape(args, tool).unwrap_or((Code::InvalidValue, ""));
        self.error(code, field, tool).caused_by(cause)
    }
    fn shape(self, args: &Map<String, Value>, tool: bool) -> Option<(Code, &'static str)> {
        if tool && args.contains_key("op") {
            return Some((Code::UnknownArgument, ""));
        }
        if self == Self::Consolidate {
            if tool && !args.contains_key("run") {
                return Some((Code::MissingArgument, "run"));
            }
            return self.consolidate_shape(args, tool);
        }
        self.fields_shape(args, self.fields(), tool, !tool)
    }
    fn consolidate_shape(
        self,
        args: &Map<String, Value>,
        tool: bool,
    ) -> Option<(Code, &'static str)> {
        let body_fields = &self.fields()[..6];
        let body = if tool {
            args
        } else {
            if args
                .keys()
                .any(|key| !["op", "request", "request_id", "run"].contains(&key.as_str()))
            {
                return Some((Code::UnknownArgument, ""));
            }
            match args.get("request") {
                None => return Some((Code::MissingArgument, "request")),
                Some(Value::Object(body)) => body,
                Some(value)
                    if crate::service::input::RawConsolidate::deserialize(value).is_ok() =>
                {
                    return self
                        .field_shape(args, "request_id", tool)
                        .or_else(|| self.field_shape(args, "run", tool));
                }
                _ => return Some((Code::InvalidValue, "request")),
            }
        };
        if let Some(problem) = self.fields_shape(body, body_fields, tool, false) {
            return Some(if !tool && problem.1.is_empty() {
                (problem.0, "request")
            } else {
                problem
            });
        }
        self.field_shape(args, "request_id", tool)
            .or_else(|| self.field_shape(args, "run", tool))
    }
    fn fields_shape(
        self,
        args: &Map<String, Value>,
        fields: &[&'static str],
        tool: bool,
        op: bool,
    ) -> Option<(Code, &'static str)> {
        if args.keys().any(|key| {
            !(fields.contains(&key.as_str())
                || (op && key == "op")
                || (tool
                    && self == Self::Consolidate
                    && ["request_id", "run"].contains(&key.as_str())))
        }) {
            return Some((Code::UnknownArgument, ""));
        }
        // Existing serde object iteration diagnoses present bad types before
        // missing fields; retain that order, with known names only.
        for field in args
            .keys()
            .filter_map(|key| fields.iter().copied().find(|field| field == key))
        {
            if let Some(problem) = self.field_shape(args, field, tool) {
                return Some(problem);
            }
        }
        fields
            .iter()
            .find_map(|field| self.field_shape(args, field, tool))
    }
    fn field_shape(
        self,
        args: &Map<String, Value>,
        field: &'static str,
        tool: bool,
    ) -> Option<(Code, &'static str)> {
        let invalid = match args.get(field) {
            None => {
                return self
                    .required(field, tool)
                    .then_some((Code::MissingArgument, field));
            }
            Some(Value::Null) if self.nullable(field) => false,
            Some(value) if field == "recopy" => !value.is_boolean(),
            Some(value) if field == "decision" => {
                crate::page::Decision::deserialize(value).is_err()
            }
            Some(Value::String(_)) => false,
            _ => true,
        };
        invalid.then_some((Code::InvalidValue, field))
    }
}
