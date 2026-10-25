use super::{Config, Error, cases};
use serde_json::{Value, json};

pub(super) fn all() -> Value {
    let mut observations = Vec::new();
    for &(name, input) in cases::COMMON {
        observations.push(json!({"name": name, "format": "json", "outcome": json(input)}));
        observations.push(json!({"name": name, "format": "yaml", "outcome": yaml(input)}));
    }
    for &(name, input) in cases::YAML {
        observations.push(json!({"name": name, "format": "yaml", "outcome": yaml(input)}));
    }
    Value::Array(observations)
}
fn success(config: &Config) -> Value {
    json!({
        "json": serde_json::to_string(config).unwrap(),
        "yaml": serde_yaml_ng::to_string(config).unwrap(),
    })
}
fn json(input: &str) -> Value {
    match serde_json::from_str::<Config>(input) {
        Ok(config) => success(&config),
        Err(error) => json!({
            "error": error.to_string(),
            "category": format!("{:?}", error.classify()),
            "line": error.line(),
            "column": error.column(),
            "public": Error::from(error),
        }),
    }
}
fn yaml(input: &str) -> Value {
    match serde_yaml_ng::from_str::<Config>(input) {
        Ok(config) => success(&config),
        Err(error) => json!({
            "error": error.to_string(),
            "location": error.location().map(|location| json!({
                "index": location.index(),
                "line": location.line(),
                "column": location.column(),
            })),
            "public": Error::from(error),
        }),
    }
}
