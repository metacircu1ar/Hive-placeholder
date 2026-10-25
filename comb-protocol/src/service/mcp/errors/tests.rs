use super::*;
use hive_input::Code;

#[test]
fn write_fields_map_to_tool_arguments_without_losing_receipt_metadata() {
    for kind in [
        Write::Put,
        Write::Append,
        Write::Capture,
        Write::Consolidate,
        Write::Decide,
        Write::Promote,
    ] {
        for field in std::iter::once("").chain(kind.fields().iter().copied()) {
            let mut original = kind.error(Code::InvalidValue, field, false).committed();
            original.request_id = Some("retry-id".into());
            let mapped = tool_error(&format!("comb_{}", kind.name()), original);
            let input = mapped.input().unwrap();
            assert_eq!(input.field(), kind.pointer(field, true));
            assert_eq!(input.examples(), &[kind.example(true)]);
            assert_eq!(input.code(), Code::InvalidValue);
            assert_eq!(mapped.request_id.as_deref(), Some("retry-id"));
            assert!(mapped.file_committed);
            assert!(std::error::Error::source(&mapped).is_some());
        }
    }
}

#[test]
fn impossible_fields_fail_operationally_without_echoing_peer_content() {
    let error = hive_input::InputError::new(
        Code::InvalidValue,
        "PRIVATE message",
        "/operation/private-field",
        "PRIVATE rule",
        vec!["PRIVATE example".into()],
    )
    .unwrap();
    let mut error = Error::from(error).committed();
    error.request_id = Some("retry-id".into());
    let mapped = tool_error("comb_capture", error);
    assert!(mapped.input().is_none());
    assert_eq!(mapped.code, Error::invalid_input_reply().code);
    assert_eq!(mapped.request_id.as_deref(), Some("retry-id"));
    assert!(mapped.file_committed);
    assert!(!serde_json::to_string(&mapped).unwrap().contains("PRIVATE"));
    let operational = Error::conflict().committed();
    let before = serde_json::to_value(&operational).unwrap();
    assert_eq!(
        serde_json::to_value(tool_error("comb_put", operational)).unwrap(),
        before
    );
}

#[test]
fn typed_import_progress_survives_generic_error_translation() {
    let mut error = Write::Put
        .error(Code::InvalidValue, "markdown", false)
        .committed();
    error.import_progress = Some(Box::new(crate::import::ImportProgress {
        batch_id: crate::values::ImportId::generate(),
        state: crate::import::ImportState::Pending,
        total: 2,
        completed: 1,
        unfinished: 1,
        unresolved: None,
    }));
    let expected = error.import_progress.clone();
    let translated = tool_error("comb_put", error);
    assert_eq!(translated.import_progress, expected);
    assert!(translated.file_committed);
}
