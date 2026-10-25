//! Agent tool schemas and argument validation.
use super::{
    AgentsArgs, AskArgs, BatchArgs, BatchItem, BatchKind, MessagesArgs, ReadArgs, SendArgs,
    SetAuthArgs, SetNameArgs, SetOptionsArgs, SpawnArgs, TargetArgs,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{borrow::Cow, collections::BTreeMap, sync::LazyLock};

mod catalog;
mod validation;
#[cfg(test)]
mod validation_tests;
pub(crate) use catalog::error;
use validation::ArgumentSchema;
pub(crate) use validation::validate_tool;

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct ToolArguments<T> {
    #[schemars(flatten)]
    _arguments: T,
}

#[derive(JsonSchema)]
struct WithRequestId<T> {
    #[serde(flatten)]
    _arguments: T,
    #[serde(rename = "request_id")]
    /// Stable retry identity: 1..128 UTF-8 bytes, no control characters.
    _request_id: String,
}

// Agent tools omit the socket's instructions switch and explicit completion target.
#[derive(JsonSchema)]
struct StatusToolArgs {
    #[schemars(rename = "target")]
    /// ID of the agent to inspect.
    _target: String,
}

#[derive(JsonSchema)]
struct CompleteToolArgs {
    #[schemars(rename = "body")]
    /// Final result and validation evidence, 1..65536 UTF-8 bytes.
    _body: String,
}

#[derive(Serialize)]
struct ToolDefinition {
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'static str,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[expect(
    clippy::expect_used,
    reason = "Every advertised argument type is a struct with an object-shaped schema."
)]
fn generated<T: JsonSchema>() -> Value {
    let mut schema = SchemaSettings::default()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator()
        .into_root_schema_for::<ToolArguments<T>>()
        .to_value();
    // The validator consumes an explicit required list, including the empty list.
    schema
        .as_object_mut()
        .expect("tool argument structs generate object schemas")
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    strip_metadata(&mut schema);
    schema
}

// Tools expose validation keywords; annotations and Rust representation bounds
// do not constrain requests. Operation handlers enforce their domain limits.
fn strip_metadata(value: &mut Value) {
    let Value::Object(object) = value else { return };
    for key in ["$schema", "title", "default", "format", "maximum"] {
        object.remove(key);
    }
    // Canonicalize integral minima without rounding or saturating other bounds.
    if let Some(minimum) = object.get_mut("minimum")
        && minimum.is_f64()
        && let Some(number) = minimum.as_f64()
        && number.fract() == 0.0
        && let Ok(integer) = format!("{number:.0}").parse::<i64>()
    {
        *minimum = Value::from(integer);
    }
    if let Some(Value::Array(required)) = object.get_mut("required") {
        required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    }
    // These are the nested schema positions emitted by our argument types.
    // Do not traverse literal property names, enum values or other user data.
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        properties.values_mut().for_each(strip_metadata);
    }
    if let Some(items) = object.get_mut("items") {
        strip_metadata(items);
    }
}

impl JsonSchema for BatchItem {
    fn schema_name() -> Cow<'static, str> {
        "BatchItem".into()
    }
    #[expect(
        clippy::expect_used,
        reason = "Batch member schemas come from known object structs with required fields."
    )]
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        // Batch items expose the operation vocabulary together. validate_tool
        // selects each operation and validates its exact arguments in order.
        let members = [
            generated::<WithRequestId<SpawnArgs>>(),
            generated::<WithRequestId<SendArgs>>(),
            generated::<WithRequestId<TargetArgs>>(),
        ];
        let members = members.map(|schema| {
            serde_json::from_value::<BatchSchema>(schema)
                .expect("generated batch member has object properties and required fields")
        });
        let mut properties = BTreeMap::new();
        let mut operation = BatchKind::json_schema(generator).to_value();
        operation["description"] = Value::from("Operation to accept: spawn, send or terminate.");
        properties.insert("op".into(), operation);
        for member in &members {
            properties.extend(member.properties.clone());
        }
        properties
            .get_mut("target")
            .expect("send and terminate have a target")["description"] = Value::from(
            "Agent ID for send or terminate, or @parent / @child (exactly one retained direct \
                child).",
        );
        let mut required = vec![Value::from("op")];
        required.extend(
            members[0]
                .required
                .iter()
                .filter(|field| members.iter().all(|member| member.required.contains(field)))
                .cloned(),
        );
        Schema::try_from(super::value(BatchSchema {
            kind: "object".into(),
            properties,
            required,
            additional_properties: false,
        }))
        .expect("generated batch schema is a JSON object")
    }
}

#[derive(Deserialize, Serialize)]
struct BatchSchema {
    #[serde(rename = "type")]
    kind: String,
    properties: BTreeMap<String, Value>,
    required: Vec<Value>,
    #[serde(rename = "additionalProperties")]
    additional_properties: bool,
}

struct ToolCatalog {
    raw: Value,
    arguments: BTreeMap<&'static str, ArgumentSchema>,
}
impl ToolDefinition {
    fn new<T: JsonSchema>(name: &'static str, description: &'static str) -> Self {
        Self {
            kind: "function",
            name,
            description: format!("{description} Example: {}", catalog::example(name)),
            input_schema: generated::<T>(),
        }
    }
}

static TOOLS: LazyLock<ToolCatalog> = LazyLock::new(build_tools);

pub(crate) fn tools() -> &'static Value {
    &TOOLS.raw
}

#[cfg(feature = "comb")]
fn evidence_tool() -> ToolDefinition {
    ToolDefinition::new::<WithRequestId<crate::runtime::collectors::Evidence>>(
        "hive_evidence",
        catalog::HIVE_EVIDENCE_DESCRIPTION,
    )
}

fn complete_tool() -> ToolDefinition {
    ToolDefinition::new::<WithRequestId<CompleteToolArgs>>(
        "hive_complete",
        catalog::HIVE_COMPLETE_DESCRIPTION,
    )
}

fn set_name_tool() -> ToolDefinition {
    ToolDefinition::new::<WithRequestId<SetNameArgs>>(
        "hive_set_name",
        catalog::HIVE_SET_NAME_DESCRIPTION,
    )
}

fn set_options_tool() -> ToolDefinition {
    ToolDefinition::new::<WithRequestId<SetOptionsArgs>>(
        "hive_set_options",
        catalog::HIVE_SET_OPTIONS_DESCRIPTION,
    )
}

fn mutation_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new::<WithRequestId<AskArgs>>("hive_ask", catalog::HIVE_ASK_DESCRIPTION),
        ToolDefinition::new::<WithRequestId<BatchArgs>>(
            "hive_batch",
            catalog::HIVE_BATCH_DESCRIPTION,
        ),
        ToolDefinition::new::<WithRequestId<SpawnArgs>>(
            "hive_spawn",
            catalog::HIVE_SPAWN_DESCRIPTION,
        ),
        ToolDefinition::new::<WithRequestId<SendArgs>>("hive_send", catalog::HIVE_SEND_DESCRIPTION),
        ToolDefinition::new::<WithRequestId<SetAuthArgs>>(
            "hive_set_auth",
            catalog::HIVE_SET_AUTH_DESCRIPTION,
        ),
        set_options_tool(),
        set_name_tool(),
        ToolDefinition::new::<WithRequestId<TargetArgs>>(
            "hive_terminate",
            catalog::HIVE_TERMINATE_DESCRIPTION,
        ),
        #[cfg(feature = "comb")]
        evidence_tool(),
        complete_tool(),
    ]
}

fn observation_tools() -> [ToolDefinition; 5] {
    [
        ToolDefinition::new::<super::OptionsCatalogArgs>(
            "hive_options_catalog",
            catalog::HIVE_OPTIONS_CATALOG_DESCRIPTION,
        ),
        ToolDefinition::new::<StatusToolArgs>("hive_status", catalog::HIVE_STATUS_DESCRIPTION),
        ToolDefinition::new::<AgentsArgs>("hive_agents", catalog::HIVE_AGENTS_DESCRIPTION),
        ToolDefinition::new::<ReadArgs>("hive_read", catalog::HIVE_READ_DESCRIPTION),
        ToolDefinition::new::<MessagesArgs>("hive_messages", catalog::HIVE_MESSAGES_DESCRIPTION),
    ]
}

#[expect(
    clippy::expect_used,
    reason = "Tool specifications use generated object argument schemas with typed properties."
)]
fn build_tools() -> ToolCatalog {
    let definitions = mutation_tools()
        .into_iter()
        .chain(observation_tools())
        .collect::<Vec<_>>();
    let arguments = definitions
        .iter()
        .map(|definition| {
            let schema = ArgumentSchema::deserialize(&definition.input_schema)
                .expect("generated tool schema has typed properties and required fields");
            (definition.name, schema)
        })
        .collect();
    ToolCatalog {
        raw: super::value(definitions),
        arguments,
    }
}

pub(crate) fn contextualize(error: crate::error::Error, name: &str) -> crate::error::Error {
    catalog::contextualize(error, name)
}

#[test]
#[cfg(feature = "comb")]
fn evidence_schema_defers_semantics_but_still_rejects_wrong_types_and_enums() {
    for (key, value) in [
        ("summary", json!("")),
        ("summary", json!("x".repeat(65537))),
        ("files", json!([""])),
        ("files", json!(["x".repeat(513)])),
        ("files", json!(vec!["f"; 65])),
        ("commit", json!("")),
    ] {
        let mut args =
            json!({"kind":"review", "summary":"public", "verdict":"findings", "request_id":"r"});
        args[key] = value;
        validate_tool("hive_evidence", &args).unwrap();
    }
    for (key, value) in [
        ("summary", json!(null)),
        ("summary", json!(7)),
        ("commit", json!(false)),
        ("files", json!([7])),
        ("files", json!({})),
        ("kind", json!("other")),
        ("verdict", json!("no-findings")),
    ] {
        let mut args =
            json!({"kind":"review", "summary":"public", "verdict":"findings", "request_id":"r"});
        args[key] = value;
        assert_eq!(
            validate_tool("hive_evidence", &args).unwrap_err().code(),
            "invalid_value"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_normalization_preserves_property_names_and_enum_values() {
        let expected = json!({
            "type":"object",
            "properties":{
                "title":{"type":"string"},
                "default":{"type":"string"},
                "choice":{"enum":[{"title":"value","default":7}]}
            },
            "required":["default","title"]
        });
        let mut schema = expected.clone();
        schema["title"] = json!("Schema title");
        schema["properties"]["title"]["default"] = json!("Field default");
        schema["required"] = json!(["title", "default"]);
        strip_metadata(&mut schema);
        assert_eq!(schema, expected);
    }

    #[test]
    fn minimum_normalization_preserves_the_numeric_bound() {
        for (minimum, expected) in [
            (json!(0.0), json!(0)),
            (json!(-0.0), json!(0)),
            (json!(1.0), json!(1)),
            (json!(-1.0), json!(-1)),
            (json!(1.75), json!(1.75)),
            (json!(-1.75), json!(-1.75)),
            (
                json!(9_007_199_254_740_993_i64),
                json!(9_007_199_254_740_993_i64),
            ),
            (json!(i64::MAX), json!(i64::MAX)),
            (json!(u64::MAX), json!(u64::MAX)),
            (json!(-9_223_372_036_854_775_808.0), json!(i64::MIN)),
            (
                json!(9_223_372_036_854_775_808.0),
                json!(9_223_372_036_854_775_808.0),
            ),
            (json!(f64::MAX), json!(f64::MAX)),
            (json!(f64::MIN), json!(f64::MIN)),
        ] {
            let mut schema = json!({"type":"integer","minimum":minimum});
            strip_metadata(&mut schema);
            assert_eq!(schema, json!({"type":"integer","minimum":expected}));
        }
    }
}
