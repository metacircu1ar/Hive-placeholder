//! Strict MCP form subset, rather than a general JSON Schema evaluator.
// Keep approval forms small enough for bounded human-facing controls.
const MAX_PROPERTIES: usize = 16;
const MAX_ENUM_VALUES: usize = 64;
// Exact f64 boundaries of the signed/unsigned integers represented by JSON numbers.
const MIN_STORED_INTEGER: f64 = -9_223_372_036_854_775_808.0;
const MAX_STORED_INTEGER_EXCLUSIVE: f64 = 18_446_744_073_709_551_616.0;

use super::{RESPONSE_BYTES, SCHEMA_BYTES, formats};
use serde_json::{Map, Number, Value};
use std::{cmp::Ordering, collections::HashSet};

const METADATA: &[&str] = &["type", "title", "description", "default"];

pub(crate) fn valid_schema(schema: &Value) -> bool {
    if schema.to_string().len() > SCHEMA_BYTES {
        return false;
    }
    let Some(root) = schema.as_object() else {
        return false;
    };
    let Some(properties) = root.get("properties").and_then(Value::as_object) else {
        return false;
    };
    root.get("type").and_then(Value::as_str) == Some("object")
        && keys(root, &["type", "properties", "required"])
        && properties.len() <= MAX_PROPERTIES
        && properties.values().all(valid_field)
        && root.get("required").is_none_or(|required| {
            let Some(names) = strings(required) else {
                return false;
            };
            unique(&names) && names.iter().all(|name| properties.contains_key(*name))
        })
}

fn keys(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}
fn strings(value: &Value) -> Option<Vec<&str>> {
    value.as_array()?.iter().map(Value::as_str).collect()
}
fn unique(values: &[&str]) -> bool {
    values.iter().copied().collect::<HashSet<_>>().len() == values.len()
}
fn choice_values(value: &Value, labeled: bool) -> Option<Vec<&str>> {
    let values = if labeled {
        value
            .as_array()?
            .iter()
            .map(|choice| {
                let object = choice.as_object()?;
                if !keys(object, &["const", "title"]) || !object.get("title")?.is_string() {
                    return None;
                }
                object.get("const")?.as_str()
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        strings(value)?
    };
    (!values.is_empty() && values.len() <= MAX_ENUM_VALUES && unique(&values)).then_some(values)
}

fn valid_field(value: &Value) -> bool {
    let Some(field) = value.as_object() else {
        return false;
    };
    if ["title", "description"]
        .iter()
        .any(|key| field.get(*key).is_some_and(|v| !v.is_string()))
    {
        return false;
    }
    let extra = match field.get("type").and_then(Value::as_str) {
        Some("string") if field.contains_key("enum") => {
            if field
                .get("enum")
                .and_then(|v| choice_values(v, false))
                .is_none()
            {
                return false;
            }
            vec!["enum"]
        }
        Some("string") if field.contains_key("oneOf") => {
            if field
                .get("oneOf")
                .and_then(|v| choice_values(v, true))
                .is_none()
            {
                return false;
            }
            vec!["oneOf"]
        }
        Some("string") => {
            if invalid_string_bounds(field) {
                return false;
            }
            vec!["minLength", "maxLength", "format"]
        }
        Some("number" | "integer") => {
            if invalid_numeric_bounds(field) {
                return false;
            }
            vec!["minimum", "maximum"]
        }
        Some("boolean") => vec![],
        Some("array") => {
            if !count_bounds(field, "minItems", "maxItems") || array_choices(field).is_none() {
                return false;
            }
            vec!["items", "minItems", "maxItems"]
        }
        _ => return false,
    };
    field
        .keys()
        .all(|key| METADATA.contains(&key.as_str()) || extra.contains(&key.as_str()))
        && field
            .get("default")
            .is_none_or(|default| field_value(field, default))
}

fn count_bounds(field: &Map<String, Value>, min: &str, max: &str) -> bool {
    [min, max]
        .iter()
        .all(|key| field.get(*key).is_none_or(|v| v.as_u64().is_some()))
        && match (
            field.get(min).and_then(Value::as_u64),
            field.get(max).and_then(Value::as_u64),
        ) {
            (Some(min), Some(max)) => min <= max,
            _ => true,
        }
}
fn count_in(field: &Map<String, Value>, min: &str, max: &str, count: usize) -> bool {
    field
        .get(min)
        .and_then(Value::as_u64)
        .is_none_or(|min| count as u64 >= min)
        && field
            .get(max)
            .and_then(Value::as_u64)
            .is_none_or(|max| count as u64 <= max)
}
fn array_choices(field: &Map<String, Value>) -> Option<Vec<&str>> {
    let items = field.get("items")?.as_object()?;
    if let Some(choices) = items.get("anyOf") {
        if !keys(items, &["anyOf"]) {
            return None;
        }
        choice_values(choices, true)
    } else {
        if !keys(items, &["type", "enum"]) || items.get("type")?.as_str()? != "string" {
            return None;
        }
        choice_values(items.get("enum")?, false)
    }
}

/// The caller validates the stored schema before matching a response.
pub(crate) fn valid_response(schema: &Value, response: &Value) -> bool {
    if response.to_string().len() > RESPONSE_BYTES {
        return false;
    }
    let Some(fields) = response.as_object() else {
        return false;
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let required = schema.get("required").and_then(Value::as_array);
    required.is_none_or(|names| {
        names
            .iter()
            .all(|name| name.as_str().is_some_and(|n| fields.contains_key(n)))
    }) && fields.iter().all(|(key, value)| {
        properties
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|field| field_value(field, value))
    })
}

fn field_value(field: &Map<String, Value>, value: &Value) -> bool {
    match field.get("type").and_then(Value::as_str) {
        Some("string") => {
            let Some(text) = value.as_str() else {
                return false;
            };
            count_in(field, "minLength", "maxLength", text.chars().count())
                && field.get("format").is_none_or(|v| {
                    v.as_str()
                        .is_some_and(|format| formats::valid(format, text))
                })
                && field.get("enum").is_none_or(|v| {
                    choice_values(v, false).is_some_and(|choices| choices.contains(&text))
                })
                && field.get("oneOf").is_none_or(|v| {
                    choice_values(v, true).is_some_and(|choices| choices.contains(&text))
                })
        }
        Some(kind @ ("number" | "integer")) => {
            let Some(number) = value.as_number() else {
                return false;
            };
            (kind != "integer"
                || number.is_i64()
                || number.is_u64()
                || number.as_f64().is_some_and(|n| n.fract() == 0.0))
                && field
                    .get("minimum")
                    .and_then(Value::as_number)
                    .is_none_or(|min| !number_cmp(number, min).is_lt())
                && field
                    .get("maximum")
                    .and_then(Value::as_number)
                    .is_none_or(|max| !number_cmp(number, max).is_gt())
        }
        Some("boolean") => value.is_boolean(),
        Some("array") => {
            let (Some(values), Some(choices)) = (strings(value), array_choices(field)) else {
                return false;
            };
            count_in(field, "minItems", "maxItems", values.len())
                && values.iter().all(|v| choices.contains(v))
        }
        _ => false,
    }
}

fn integer(number: &Number) -> Option<i128> {
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
}
#[expect(
    clippy::expect_used,
    reason = "JSON numbers contain finite integers or floats that convert to finite f64."
)]
fn number_cmp(a: &Number, b: &Number) -> Ordering {
    match (integer(a), integer(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (None, Some(b)) => float_integer_cmp(a.as_f64().expect("JSON number"), b),
        (Some(a), None) => float_integer_cmp(b.as_f64().expect("JSON number"), a).reverse(),
        (None, None) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .expect("finite JSON numbers"),
    }
}
// Only cast after checking the entire JSON integer range. Compare the fractional
// remainder separately, avoiding f64 rounding of integer bounds above 2^53.
#[allow(clippy::cast_possible_truncation)]
#[expect(
    clippy::expect_used,
    reason = "A finite JSON number has a fractional part with a total comparison."
)]
fn float_integer_cmp(float: f64, integer: i128) -> Ordering {
    if float < MIN_STORED_INTEGER {
        return Ordering::Less;
    }
    if float >= MAX_STORED_INTEGER_EXCLUSIVE {
        return Ordering::Greater;
    }
    (float as i128)
        .cmp(&integer)
        .then_with(|| float.fract().partial_cmp(&0.0).expect("finite number"))
}

fn invalid_string_bounds(field: &Map<String, Value>) -> bool {
    !count_bounds(field, "minLength", "maxLength")
        || field
            .get("format")
            .is_some_and(|v| !matches!(v.as_str(), Some("email" | "uri" | "date" | "date-time")))
}

fn invalid_numeric_bounds(field: &Map<String, Value>) -> bool {
    ["minimum", "maximum"]
        .iter()
        .any(|key| field.get(*key).is_some_and(|v| !v.is_number()))
        || matches!(
            (
                field.get("minimum").and_then(Value::as_number),
                field.get("maximum").and_then(Value::as_number),
            ),
            (Some(min), Some(max)) if number_cmp(min, max).is_gt()
        )
}

#[cfg(test)]
mod tests;
