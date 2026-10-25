use crate::{ErrorCode, Page};
use serde_json::{Value, json};

fn parse(metadata: &Value) -> crate::Result<Page> {
    let yaml = serde_yaml_ng::to_string(metadata).unwrap();
    Page::parse(&format!("---\n{yaml}---\nBody."))
}

#[test]
fn authored_text_limits_count_utf8_bytes() {
    for (field, bytes, list) in [
        ("title", 1024, false),
        ("type", 256, false),
        ("tags", 256, true),
        ("aliases", 256, true),
    ] {
        let valid = "é".repeat(bytes / 2);
        for (text, accepted) in [(valid.clone(), true), (format!("{valid}x"), false)] {
            let mut metadata = json!({"title":"Title"});
            metadata[field] = if list { json!([text]) } else { json!(text) };
            let result = parse(&metadata);
            if accepted {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().code, ErrorCode::InvalidPage);
            }
        }
    }
}

#[test]
fn tag_and_alias_count_limits_are_independent_of_text_byte_bounds() {
    for field in ["tags", "aliases"] {
        for (count, accepted) in [(64, true), (65, false)] {
            let mut metadata = json!({"title":"Title"});
            metadata[field] = json!(vec!["é"; count]);
            let result = parse(&metadata);
            if accepted {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().code, ErrorCode::InvalidPage);
            }
        }
    }
}
