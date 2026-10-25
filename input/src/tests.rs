#![allow(clippy::unwrap_used)]
use crate::{Code, InputError, InvalidMetadata, Product, validate};
use serde_json::{Value, json};

#[test]
fn content_bytes_and_absolute_path_shapes_are_checked_without_io() {
    for length in [0, 1, 65536] {
        assert!(validate::byte_count(length, 65536).is_ok());
    }
    assert_eq!(
        validate::byte_count(65537, 65536).unwrap_err().code(),
        Code::OutOfRange
    );
    for path in ["", ".", "relative", "/private-canary\0"] {
        let rule = validate::absolute_path(std::path::Path::new(path)).unwrap_err();
        assert_eq!(rule.code(), Code::InvalidPath);
        assert!(!rule.to_string().contains("private-canary"));
    }
    assert!(validate::absolute_path(std::path::Path::new("/not-required-to-exist")).is_ok());
}

fn error() -> InputError {
    InputError::new(
        Code::InvalidDuration,
        "Invalid delay.",
        "--delay",
        "1s..7d",
        vec!["hivectl send worker --body-file task.txt --delay 1m".into()],
    )
    .unwrap()
}

#[test]
fn exact_cli_contract_and_shared_examples() {
    let rendered = error()
        .with_public_received("8d")
        .render(Product::Hive)
        .unwrap();
    assert_eq!(rendered.exit_code, 2);
    assert_eq!(
        rendered.stdout,
        concat!(
            "{\"error\":{\"code\":\"invalid_duration\",\"message\":\"Invalid delay.\",",
            "\"field\":\"--delay\",\"expected\":\"1s..7d\",",
            "\"examples\":[\"hivectl send worker --body-file task.txt --delay 1m\"],\"received\":\"8d\"}}\n"
        )
    );
    assert_eq!(
        rendered.stderr,
        concat!(
            "--delay: Invalid delay. Expected: 1s..7d\nReceived: 8d\nExamples:\n",
            "  hivectl send worker --body-file task.txt --delay 1m\n"
        )
    );
}

#[test]
fn comb_stdout_and_wire_never_include_admitted_input() {
    let public_error = error().with_public_received("public-canary");
    let rendered = public_error.render(Product::Comb).unwrap();
    assert!(!rendered.stdout.contains("public-canary"));
    assert!(!rendered.stdout.contains("received"));
    assert!(rendered.stderr.contains("Received: public-canary\n"));
    let wire = serde_json::to_string(&public_error.wire()).unwrap();
    assert!(!wire.contains("public-canary"));
    assert!(!wire.contains("received"));
    let decoded: InputError = serde_json::from_str(&wire).unwrap();
    assert_eq!(decoded, error());
    assert!(
        !decoded
            .render(Product::Hive)
            .unwrap()
            .stdout
            .contains("received")
    );
}

#[test]
fn input_is_omitted_by_default_in_all_outputs() {
    for product in [Product::Hive, Product::Comb] {
        let output = error().render(product).unwrap();
        assert!(!output.stdout.contains("received"));
        assert!(!output.stderr.contains("Received"));
    }
}

#[test]
fn strict_wire_rejects_extra_missing_duplicate_and_malformed_fields() {
    let positional = json!([
        "invalid_duration",
        "Invalid delay.",
        "--delay",
        "1s..7d",
        ["hivectl send worker --body-file task.txt --delay 1m"]
    ]);
    assert!(serde_json::from_value::<InputError>(positional).is_err());
    let wire = serde_json::to_value(error().wire()).unwrap();
    for (field, value) in [
        ("received", json!("private")),
        ("extra", json!(true)),
        ("code", json!("unknown")),
        ("examples", json!([])),
        ("examples", json!(["a", "b", "c", "d"])),
        ("examples", json!(["bad\nexample"])),
        ("field", json!("--delay\0")),
        ("expected", Value::Null),
        ("message", json!("")),
    ] {
        let mut malformed = wire.clone();
        malformed[field] = value;
        assert!(
            serde_json::from_value::<InputError>(malformed).is_err(),
            "{field}"
        );
    }
    for field in ["code", "message", "field", "expected", "examples"] {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<InputError>(missing).is_err());
    }
    let duplicate =
        serde_json::to_string(&wire)
            .unwrap()
            .replacen('{', "{\"code\":\"invalid_value\",", 1);
    assert!(serde_json::from_str::<InputError>(&duplicate).is_err());
}

#[test]
fn metadata_bounds_enforced_on_construction_and_decode() {
    let fields = [
        ("message", 1024, InvalidMetadata::Message),
        ("field", 256, InvalidMetadata::Field),
        ("expected", 1024, InvalidMetadata::Expected),
        ("examples", 2048, InvalidMetadata::Examples),
    ];
    for (field, limit, expected) in fields {
        for value in [
            String::new(),
            " \t".into(),
            "bad\u{1b}[31m".into(),
            "é".repeat(limit / 2 + 1),
        ] {
            let mut wire = serde_json::to_value(error().wire()).unwrap();
            wire[field] = if field == "examples" {
                json!([value])
            } else {
                json!(value)
            };
            let failure = serde_json::from_value::<InputError>(wire).unwrap_err();
            assert!(failure.to_string().contains(&expected.to_string()));
        }
        let value = "é".repeat(limit / 2);
        let mut wire = serde_json::to_value(error().wire()).unwrap();
        wire[field] = if field == "examples" {
            json!([value])
        } else {
            json!(value)
        };
        assert!(serde_json::from_value::<InputError>(wire).is_ok());
    }
    assert_eq!(
        InputError::new(Code::InvalidValue, "message", "field", "expected", vec![]),
        Err(InvalidMetadata::Examples)
    );
    let examples = vec!["first".into(), "second".into(), "third".into()];
    assert!(InputError::new(Code::InvalidValue, "message", "field", "expected", examples).is_ok());
}

fn received(value: &str) -> String {
    let output = error()
        .with_public_received(value)
        .render(Product::Hive)
        .unwrap();
    let json: Value = serde_json::from_str(&output.stdout).unwrap();
    json["error"]["received"].as_str().unwrap().into()
}

#[test]
fn received_escapes_controls_and_preserves_unicode_with_bounded_allocation() {
    assert_eq!(
        received("a\n\r\t\0\u{1b}\\é"),
        "a\\n\\r\\t\\u{0}\\u{1b}\\\\é"
    );
    assert_eq!(received(&"a".repeat(200)), "a".repeat(200));
    assert_eq!(
        received(&"a".repeat(201)),
        format!("{}...", "a".repeat(197))
    );
    assert_eq!(received(&"é".repeat(100)), "é".repeat(100));
    assert_eq!(received(&"é".repeat(101)), format!("{}...", "é".repeat(98)));
    let controls = received(&"\u{1b}".repeat(1_000_000));
    assert!(controls.len() <= 200);
    assert!(controls.ends_with("}...")); // never split an escape sequence
    for input in ["🙂".repeat(100), "x\n".repeat(100), "a".repeat(199) + "é"] {
        let text = received(&input);
        assert!(text.len() <= 200);
        assert!(!text.chars().any(char::is_control));
        assert!(text.ends_with("..."));
    }
}

#[test]
fn request_and_run_boundaries_are_utf8_bytes() {
    assert!(validate::request_id(&"a".repeat(128)).is_ok());
    assert!(validate::request_id(&"é".repeat(64)).is_ok());
    for value in [
        String::new(),
        "a".repeat(129),
        "é".repeat(65),
        "has\nnewline".into(),
        "nul\0".into(),
    ] {
        assert_eq!(
            validate::request_id(&value).unwrap_err().code(),
            Code::InvalidRequestId
        );
    }
    assert!(validate::run_name("éé", 4).is_ok());
    assert_eq!(
        validate::run_name("éé", 3).unwrap_err().code(),
        Code::InvalidRunName
    );
    assert!(validate::run_name("x", 0).is_err());
}

#[test]
fn agent_names_are_bounded_labels_without_controls_or_surrounding_spaces() {
    for name in ["reviewer", "Build bot 2", "é".repeat(32).as_str(), "a"] {
        assert!(validate::agent_name(name).is_ok(), "{name}");
    }
    assert!(validate::agent_name(&"a".repeat(64)).is_ok());
    for name in [
        String::new(),
        "a".repeat(65),
        "é".repeat(33),
        " lead".into(),
        "lead ".into(),
        "\tlead".into(),
        "two\nlines".into(),
        " ".into(),
    ] {
        assert_eq!(
            validate::agent_name(&name).unwrap_err().code(),
            Code::InvalidValue,
            "{name:?}"
        );
    }
}

#[test]
fn documented_models_allowed_but_options_controls_and_overlong_tokens_rejected() {
    for model in [
        "sonnet[1m]",
        "gpt-5.6-luna",
        "claude-opus-4-1@20250805",
        "vendor/model:revision",
        &"a".repeat(128),
    ] {
        assert!(validate::model(model).is_ok(), "{model}");
    }
    for model in [
        "",
        "-x",
        "--dangerously-skip-permissions",
        "a b",
        "a\0b",
        "模型",
        &"a".repeat(129),
    ] {
        let violation = validate::model(model).unwrap_err();
        assert_eq!(violation.code(), Code::InvalidModel);
    }
}

#[test]
fn effort_closed_list_and_uuid_follow_existing_syntax() {
    for effort in [
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ] {
        assert!(validate::effort(effort).is_ok());
    }
    for effort in ["", "High", "medium\n", "unknown"] {
        assert_eq!(
            validate::effort(effort).unwrap_err().code(),
            Code::InvalidEffort
        );
    }
    for uuid in [
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        "6BA7B8109DAD11D180B400C04FD430C8",
        "urn:uuid:6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    ] {
        assert!(validate::uuid(uuid).is_ok());
    }
    for uuid in [
        "",
        "human",
        "../../state",
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8\n",
    ] {
        assert_eq!(validate::uuid(uuid).unwrap_err().code(), Code::InvalidUuid);
    }
}

#[test]
fn duration_units_bounds_and_overflow() {
    for (value, seconds) in [
        ("1s", 1),
        ("2m", 120),
        ("3h", 10_800),
        ("4d", 345_600),
        ("1w", 604_800),
    ] {
        assert_eq!(validate::duration(value, 1..=604_800).unwrap(), seconds);
    }
    for value in [
        "",
        "1",
        "s",
        "0s",
        "-1s",
        "+1s",
        "1.5h",
        " 1h",
        "1h ",
        "1S",
        "é",
        "🙂s",
        "8d",
        "18446744073709551616s",
        "18446744073709551615w",
    ] {
        assert_eq!(
            validate::duration(value, 1..=604_800).unwrap_err().code(),
            Code::InvalidDuration
        );
    }
    assert!(validate::duration("59s", 60..=120).is_err());
    assert!(validate::duration("60s", 60..=120).is_ok());
    assert!(validate::duration("120s", 60..=120).is_ok());
    assert!(validate::duration("121s", 60..=120).is_err());
    assert!(validate::duration("0s", 0..=120).is_err());
    assert_eq!(
        validate::duration("18446744073709551615s", 1..=u64::MAX).unwrap(),
        u64::MAX
    );
}

#[test]
fn unsigned_rejects_nondecimal_overflow_and_outside_caller_bounds() {
    assert_eq!(validate::unsigned("0", 0..=100).unwrap(), 0);
    assert_eq!(validate::integer(0, 0..=100).unwrap(), 0);
    assert_eq!(validate::integer(100, 0..=100).unwrap(), 100);
    assert_eq!(
        validate::integer(101, 0..=100).unwrap_err().code(),
        Code::OutOfRange
    );
    assert_eq!(validate::unsigned("100", 0..=100).unwrap(), 100);
    assert_eq!(
        validate::unsigned("18446744073709551615", 0..=u64::MAX).unwrap(),
        u64::MAX
    );
    for value in [
        "",
        "+1",
        "-1",
        "1.0",
        "1e2",
        " 1",
        "١",
        "18446744073709551616",
    ] {
        assert_eq!(
            validate::unsigned(value, 0..=u64::MAX).unwrap_err().code(),
            Code::InvalidValue
        );
    }
    assert_eq!(
        validate::unsigned("101", 0..=100).unwrap_err().code(),
        Code::OutOfRange
    );
    assert_eq!(
        validate::unsigned("0", 1..=100).unwrap_err().code(),
        Code::OutOfRange
    );
}

#[test]
fn violations_never_echo_rejected_values() {
    let private = "SECRET-CANARY\n";
    for violation in [
        validate::request_id(private).unwrap_err(),
        validate::run_name(private, 512).unwrap_err(),
        validate::agent_name(private).unwrap_err(),
        validate::model(private).unwrap_err(),
        validate::effort(private).unwrap_err(),
        validate::uuid(private).unwrap_err(),
        validate::unsigned(private, 0..=100).unwrap_err(),
        validate::duration(private, 1..=604_800).unwrap_err(),
    ] {
        assert!(!violation.to_string().contains("SECRET"));
        assert!(!format!("{violation:?}").contains("SECRET"));
    }
}

#[test]
fn opaque_text_and_configuration_names_keep_distinct_grammars() {
    for value in ["a", " ", "\n", "éé"] {
        assert!(validate::text_bytes(value, 4).is_ok());
    }
    for value in ["", "ééé", "abcde"] {
        assert_eq!(
            validate::text_bytes(value, 4).unwrap_err().code(),
            Code::OutOfRange
        );
    }
    for value in ["a", "_", "-", "Main-1_profile"] {
        assert!(validate::config_name(value).is_ok());
    }
    assert!(validate::config_name(&"a".repeat(64)).is_ok());
    for value in [
        String::new(),
        "a".repeat(65),
        "é".into(),
        "a.b".into(),
        "a b".into(),
        "a\n".into(),
    ] {
        assert_eq!(
            validate::config_name(&value).unwrap_err().code(),
            Code::InvalidValue
        );
    }
}
