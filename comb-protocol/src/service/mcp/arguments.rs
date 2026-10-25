//! Shared hosted and stdio tool argument conversion.
mod schema;
mod write;

#[cfg(test)]
mod tests;

use crate::service::input::{
    mutation::{self, Write},
    retrieval::{self, Read},
};
use crate::{Error, Result, service::Operation};
use hive_input::Code;
use serde::de::{DeserializeOwned, value::MapDeserializer};
use serde_json::{Map, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Search,
    Get,
    Put,
    Append,
    Capture,
    Consolidate,
    List,
    Links,
    Backlinks,
    Graph,
    Decide,
    Promote,
}
impl Tool {
    pub const ALL: [Self; 12] = [
        Self::Search,
        Self::Get,
        Self::Put,
        Self::Append,
        Self::Capture,
        Self::Consolidate,
        Self::List,
        Self::Links,
        Self::Backlinks,
        Self::Graph,
        Self::Decide,
        Self::Promote,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Get => "get",
            Self::Put => "put",
            Self::Append => "append",
            Self::Capture => "capture",
            Self::Consolidate => "consolidate",
            Self::List => "list",
            Self::Links => "links",
            Self::Backlinks => "backlinks",
            Self::Graph => "graph",
            Self::Decide => "decide",
            Self::Promote => "promote",
        }
    }

    fn parse(name: &str) -> Result<Self> {
        let name = name.strip_prefix("comb_").ok_or_else(unknown_tool)?;
        Self::ALL
            .into_iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(unknown_tool)
    }

    fn arguments(self, args: &Map<String, Value>, call_id: Option<&str>) -> Result<Operation> {
        if let Some(mutation) = Write::named(self.name()) {
            return self.write_arguments(args, call_id).map_err(|cause| {
                if cause.input().is_some() {
                    cause
                } else {
                    mutation.shape_error(args, true, cause)
                }
            });
        }
        match self {
            Self::Search => Read::Search.tool(args),
            Self::List => Read::List.tool(args),
            Self::Get => Read::Get.tool(args),
            Self::Links => Read::Links.tool(args),
            Self::Backlinks => Read::Backlinks.tool(args),
            Self::Graph => Read::Graph.tool(args),
            _ => Err(Error::internal_serialization()),
        }
    }

    fn write_arguments(
        self,
        args: &Map<String, Value>,
        call_id: Option<&str>,
    ) -> Result<Operation> {
        if args.contains_key("op") {
            return Err(Error::op_argument_forbidden());
        }
        let raw = match self {
            Self::Put => decode::<write::Put>(args)?.raw(call_id),
            Self::Append => decode::<write::Append>(args)?.raw(call_id),
            Self::Capture => decode::<write::Capture>(args)?.raw(call_id),
            Self::Promote => decode::<write::Promote>(args)?.raw(call_id),
            Self::Decide => decode::<write::Decide>(args)?.raw(call_id),
            Self::Consolidate => write::Consolidate::parse(args, call_id)?.raw(),
            _ => return Err(Error::internal_serialization()),
        };
        mutation::operation(raw, true)
    }

    pub fn input_schema(self) -> Value {
        let mut schema = match self {
            Self::Search => schema::retrieval(Read::Search),
            Self::List => schema::retrieval(Read::List),
            Self::Get => schema::retrieval(Read::Get),
            Self::Links => schema::retrieval(Read::Links),
            Self::Backlinks => schema::retrieval(Read::Backlinks),
            Self::Graph => schema::retrieval(Read::Graph),
            Self::Put => schema::generate::<write::Put>(&["slug", "markdown"]),
            Self::Append => schema::generate::<write::Append>(&["slug", "text", "source"]),
            Self::Capture => schema::generate::<write::Capture>(&["text"]),
            Self::Consolidate => schema::generate::<write::Consolidate>(&[
                "note",
                "target",
                "note_hash",
                "target_hash",
                "summary",
                "run",
            ]),
            Self::Decide => {
                schema::generate::<write::Decide>(&["slug", "expected_hash", "decision", "reason"])
            }
            Self::Promote => schema::generate::<write::Promote>(&["slug", "expected_hash"]),
        };
        if let Some(mutation) = Write::named(self.name()) {
            schema::mutation(&mut schema, mutation);
        }
        schema
    }
}

/// The call ID supplied by `Hive` is stable across provider retransmission.
/// Standalone clients may supply `request_id`; otherwise it is returned on failure.
pub fn operation(name: &str, arguments: Value, call_id: Option<&str>) -> Result<Operation> {
    let args = arguments.as_object().ok_or_else(|| {
        retrieval::error(
            Code::InvalidValue,
            "/arguments",
            "a tool argument object",
            name.strip_prefix("comb_")
                .and_then(Write::named)
                .filter(|m| *m != Write::Remove)
                .map_or_else(
                    || Read::Search.example(true),
                    |mutation| mutation.example(true),
                ),
        )
    })?;
    Tool::parse(name)?.arguments(args, call_id)
}

fn decode<T: DeserializeOwned>(args: &Map<String, Value>) -> Result<T> {
    Ok(T::deserialize(args)?)
}

// The staged mutation envelope omits only its already-handled fields.
// Map iteration retains Value's ordering and serde_json's Data/zero-coordinate errors.
fn project<T: DeserializeOwned>(args: &Map<String, Value>, handled: &[&str]) -> Result<T> {
    let fields = args
        .iter()
        .filter(|(key, _)| !handled.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), value));
    Ok(T::deserialize(
        MapDeserializer::<_, serde_json::Error>::new(fields),
    )?)
}

fn unknown_tool() -> Error {
    retrieval::error(
        Code::InvalidValue,
        "/name",
        "an advertised comb_* tool name from tools/list",
        Read::Search.example(true),
    )
}
