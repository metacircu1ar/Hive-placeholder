//! Mutation tool envelopes retain missing request IDs without accepting null IDs.
use super::{project, schema};
use crate::{
    Error, Result,
    page::Decision,
    service::input::{RawConsolidate, RawOperation},
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

#[derive(Default)]
struct Identity(Option<String>);
impl<'de> Deserialize<'de> for Identity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}
impl Identity {
    fn resolve(self, call_id: Option<&str>) -> String {
        self.0.unwrap_or_else(|| {
            call_id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned)
        })
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Put {
    slug: String,
    markdown: String,
    #[serde(default)]
    #[schemars(with = "String")]
    expected_hash: Option<String>,
    #[serde(default)]
    #[schemars(!default, with = "String")]
    request_id: Identity,
    #[serde(default)]
    #[schemars(with = "String")]
    run: Option<String>,
}
impl Put {
    pub fn raw(self, call_id: Option<&str>) -> RawOperation {
        RawOperation::Put {
            slug: self.slug,
            markdown: self.markdown,
            expected_hash: self.expected_hash,
            request_id: self.request_id.resolve(call_id),
            run: self.run,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Append {
    slug: String,
    text: String,
    source: String,
    #[serde(default)]
    #[schemars(with = "String")]
    summary: Option<String>,
    #[serde(default)]
    #[schemars(with = "String")]
    expected_hash: Option<String>,
    #[serde(default)]
    #[schemars(!default, with = "String")]
    request_id: Identity,
    #[serde(default)]
    #[schemars(with = "String")]
    run: Option<String>,
}
impl Append {
    pub fn raw(self, call_id: Option<&str>) -> RawOperation {
        RawOperation::Append {
            slug: self.slug,
            text: self.text,
            source: self.source,
            summary: self.summary,
            expected_hash: self.expected_hash,
            request_id: self.request_id.resolve(call_id),
            run: self.run,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    text: String,
    #[serde(default)]
    #[schemars(!default, with = "String")]
    request_id: Identity,
    #[serde(default)]
    #[schemars(with = "String")]
    run: Option<String>,
}
impl Capture {
    pub fn raw(self, call_id: Option<&str>) -> RawOperation {
        RawOperation::Capture {
            text: self.text,
            request_id: self.request_id.resolve(call_id),
            run: self.run,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Promote {
    slug: String,
    expected_hash: String,
    #[serde(default)]
    #[schemars(!default, with = "String")]
    request_id: Identity,
    #[serde(default)]
    #[schemars(with = "String")]
    run: Option<String>,
}
impl Promote {
    pub fn raw(self, call_id: Option<&str>) -> RawOperation {
        RawOperation::Promote {
            slug: self.slug,
            expected_hash: self.expected_hash,
            request_id: self.request_id.resolve(call_id),
            run: self.run,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decide {
    slug: String,
    expected_hash: String,
    #[schemars(schema_with = "schema::enumeration::<Decision>")]
    decision: Decision,
    reason: String,
    #[serde(default)]
    #[schemars(!default, with = "String")]
    request_id: Identity,
    #[serde(default)]
    #[schemars(with = "String")]
    run: Option<String>,
}
impl Decide {
    pub fn raw(self, call_id: Option<&str>) -> RawOperation {
        RawOperation::Decide {
            slug: self.slug,
            expected_hash: self.expected_hash,
            decision: self.decision,
            reason: self.reason,
            request_id: self.request_id.resolve(call_id),
            run: self.run,
        }
    }
}

#[derive(JsonSchema)]
pub struct Consolidate {
    #[schemars(flatten)]
    request: RawConsolidate,
    request_id: String,
    run: String,
}
impl Consolidate {
    pub fn parse(args: &Map<String, Value>, call_id: Option<&str>) -> Result<Self> {
        let run = args
            .get("run")
            .ok_or_else(Error::consolidation_run_missing)?;
        // Body structure wins over request ID/run structure; all structure wins
        // over the shared domain conversion's note/target/hash/id/run checks.
        let request = project(args, &["request_id", "run"])?;
        let request_id = args
            .get("request_id")
            .map(Identity::deserialize)
            .transpose()?
            .unwrap_or_default()
            .resolve(call_id);
        let run = String::deserialize(run)?;
        Ok(Self {
            request,
            request_id,
            run,
        })
    }
    pub fn raw(self) -> RawOperation {
        RawOperation::Consolidate {
            request: self.request,
            request_id: self.request_id,
            run: self.run,
        }
    }
}
