use super::*;

pub(super) fn observe(c: &Core, raw: Value) -> crate::error::Result<Value> {
    crate::runtime::read_api::dispatch(
        c.store(),
        &RequestEnvelope::new(raw).request,
        None,
        c.config(),
    )
}

fn assert_page_error(result: crate::error::Result<Value>, code: &str, field: &str, op: &str) {
    let error = serde_json::to_value(result.unwrap_err()).unwrap();
    assert_eq!(error["code"], code);
    assert_eq!(error["field"], format!("/{field}"));
    assert!(!error["expected"].as_str().unwrap().is_empty());
    let example: Value = serde_json::from_str(error["examples"][0].as_str().unwrap()).unwrap();
    assert_eq!(example["op"], op);
    assert!(error.get("received").is_none());
}

#[test]
fn read_dispatch_rejects_limits_and_wrong_types_before_target_lookup() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    for base in [
        json!({"op":"agents"}),
        json!({"op":"agents", "stalled_seconds":1}),
        json!({"op":"messages", "target":"missing"}),
        json!({"op":"read", "target":"missing"}),
    ] {
        for value in [
            json!(0),
            json!(201),
            json!(u64::MAX),
            json!(null),
            json!("100"),
            json!(true),
        ] {
            let mut raw = base.clone();
            let code = if value.is_u64() {
                "out_of_range"
            } else {
                "invalid_value"
            };
            raw["limit"] = value;
            assert_page_error(
                observe(&c, raw),
                code,
                "limit",
                base["op"].as_str().unwrap(),
            );
        }
    }
    for base in [
        json!({"op":"read", "target":"artifact:missing"}),
        json!({"op":"output", "id":"missing"}),
    ] {
        for value in [
            json!(0),
            json!(3),
            json!(65537),
            json!(u64::MAX),
            json!(null),
            json!("4"),
        ] {
            let mut raw = base.clone();
            let code = if value.is_u64() {
                "out_of_range"
            } else {
                "invalid_value"
            };
            raw["limit"] = value;
            assert_page_error(
                observe(&c, raw),
                code,
                "limit",
                base["op"].as_str().unwrap(),
            );
        }
    }
    for (mut raw, field) in [
        (json!({"op":"agents"}), "after"),
        (json!({"op":"read", "target":"missing"}), "after"),
        (json!({"op":"messages", "target":"missing"}), "before"),
        (json!({"op":"output", "id":"missing"}), "offset"),
    ] {
        raw[field] = json!("0");
        let op = raw["op"].as_str().unwrap().to_owned();
        assert_page_error(observe(&c, raw), "invalid_value", field, &op);
    }
}

#[test]
fn read_defaults_and_explicit_join_pagination_remain_distinct() {
    for (target, expected) in [("agent", 100), ("artifact:ID", 65536)] {
        let args: ReadArgs = serde_json::from_value(json!({"target":target})).unwrap();
        let page = args.page().unwrap();
        assert_eq!(page.after, 0);
        assert_eq!(page.limit, expected);
    }
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    for (field, value) in [
        ("after", json!(0)),
        ("limit", json!(100)),
        ("limit", json!(null)),
    ] {
        let mut raw = json!({"op":"read", "target":"join:missing"});
        raw[field] = value;
        assert_page_error(observe(&c, raw), "conflicting_arguments", field, "read");
    }
    c.store().save(&agent("a")).unwrap();
    for before in [None, Some(Value::Null), Some(json!(u64::MAX))] {
        let mut raw = json!({"op":"messages", "target":"a"});
        if let Some(before) = before {
            raw["before"] = before;
        }
        assert!(observe(&c, raw).is_ok());
    }
}

#[test]
fn artifact_default_and_minimum_pages_preserve_utf8_progress() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    let a = agent("a");
    c.store().save(&a).unwrap();
    c.store()
        .event(
            &a,
            crate::runtime::domain::EventKind::AssistantMessage,
            json!({"text":"💡".repeat(5000)}),
        )
        .unwrap();
    let events = c.store().events("a", 0, 100).unwrap();
    let id = events["events"][0]["data"]["artifact"]["id"]
        .as_str()
        .unwrap();
    let page = observe(&c, json!({"op":"read", "target":format!("artifact:{id}")})).unwrap();
    let text = page["text"].as_str().unwrap();
    assert!(text.len() > 100 && text.len() <= 65536);
    let offset = text.find('💡').unwrap();
    let minimum = c.store().output(id, offset as u64, 4).unwrap();
    assert_eq!(minimum["text"], "💡");
    assert_eq!(minimum["next_offset"], offset + 4);
    for limit in [0, 3, 65537, usize::MAX] {
        assert_eq!(
            c.store().output(id, 0, limit).unwrap_err().code(),
            "invalid_request"
        );
    }
    for limit in [0, 201, u64::MAX] {
        for result in [
            c.store().events("missing", 0, limit),
            c.store().messages("missing", None, limit),
            c.store().agent_page(0, limit, None, None),
        ] {
            assert_eq!(result.unwrap_err().code(), "invalid_request");
        }
    }
}
