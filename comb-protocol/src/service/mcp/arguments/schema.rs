//! Adapt generated field schemas to the existing advertised tool contract.
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value};

pub fn generate<T: JsonSchema>(required: &[&str]) -> Value {
    let settings = SchemaSettings::draft07().with(|settings| {
        settings.meta_schema = None;
        settings.inline_subschemas = true;
    });
    let mut schema = settings.into_generator().into_root_schema_for::<T>();
    schema.insert(
        "required".into(),
        Value::Array(
            required
                .iter()
                .map(|s| Value::String((*s).into()))
                .collect(),
        ),
    );
    schema.insert("additionalProperties".into(), Value::Bool(false));
    let mut value = schema.to_value();
    strip_annotations(&mut value);
    value
}

// Fields use the actual parser's enum and its existing ValueEnum metadata. The
// advertised enum deliberately has no extra "type" member.
pub fn enumeration<T: clap::ValueEnum + 'static>(_: &mut SchemaGenerator) -> Schema {
    let values = T::value_variants()
        .iter()
        .filter_map(clap::ValueEnum::to_possible_value)
        .map(|value| Value::String(value.get_name().into()))
        .collect();
    Map::from_iter([("enum".into(), Value::Array(values))]).into()
}

fn strip_annotations(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for annotation in ["$schema", "title", "description", "default", "format"] {
                fields.remove(annotation);
            }
            for child in fields.values_mut() {
                strip_annotations(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_annotations(child);
            }
        }
        _ => {}
    }
}

/// Descriptions, accepted fields and examples share the decoder's registry.
pub fn retrieval(read: crate::service::input::retrieval::Read) -> Value {
    use crate::service::input::retrieval::Read;
    let properties: Map<String, Value> = read
        .fields()
        .iter()
        .map(|field| {
            let base = match *field {
                "limit" => Property {
                    kind: Some("integer"),
                    minimum: Some(1),
                    maximum: Some(if read == Read::Search {
                        crate::search::limits::SEARCH_LIMIT_MAX
                    } else {
                        crate::limits::LIST_LIMIT_MAX
                    }),
                    ..Property::default()
                },
                "depth" => Property {
                    kind: Some("integer"),
                    minimum: Some(0),
                    maximum: Some(crate::limits::GRAPH_DEPTH_MAX),
                    ..Property::default()
                },
                "unverified" => Property::kind("boolean"),
                "mode" => Property {
                    choices: Some(["text", "vector", "hybrid"]),
                    ..Property::default()
                },
                _ => Property::kind("string"),
            };
            let nullable = matches!(*field, "kind" | "tag" | "prefix" | "after" | "before")
                || (*field == "limit" && read == Read::List);
            let mut property = if nullable {
                Property {
                    any_of: Some([value(base), value(Property::kind("null"))]),
                    ..Property::default()
                }
            } else {
                base
            };
            property.description = Some(read.expected(field));
            ((*field).into(), value(property))
        })
        .collect();
    let required: &[&str] = match read {
        Read::Search => &["query"],
        Read::List => &[],
        _ => &["slug"],
    };
    value(ObjectSchema {
        kind: "object",
        properties,
        required,
        additional_properties: false,
        examples: [read.sample()],
    })
}

/// Annotate native generated types without changing their structural authority.
pub fn mutation(schema: &mut Value, mutation: crate::service::input::mutation::Write) {
    if let Some(properties) = schema["properties"].as_object_mut() {
        for (field, property) in properties {
            if mutation.nullable(field) {
                *property = value(Nullable {
                    any_of: [std::mem::take(property), value(Property::kind("null"))],
                });
            }
            property["description"] = mutation.expected(field).into();
        }
    }
    schema["examples"] = Value::Array(vec![mutation.sample()]);
}

use crate::service::input::samples::value;
#[derive(Default, serde::Serialize)]
struct Property {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum: Option<usize>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    choices: Option<[&'static str; 3]>,
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    any_of: Option<[Value; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'static str>,
}
impl Property {
    fn kind(kind: &'static str) -> Self {
        Self {
            kind: Some(kind),
            ..Self::default()
        }
    }
}
#[derive(serde::Serialize)]
struct Nullable {
    #[serde(rename = "anyOf")]
    any_of: [Value; 2],
}
#[derive(serde::Serialize)]
struct ObjectSchema<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    properties: Map<String, Value>,
    required: &'a [&'a str],
    #[serde(rename = "additionalProperties")]
    additional_properties: bool,
    examples: [Value; 1],
}
