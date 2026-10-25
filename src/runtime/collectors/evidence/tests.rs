use super::*;
use serde_json::json;

fn review() -> Evidence {
    Evidence {
        kind: EvidenceKind::Review,
        summary: "Public finding.".into(),
        files: vec![],
        commit: None,
        verdict: Some(Verdict::Findings),
    }
}
fn rejected(evidence: &Evidence, human: bool, code: &str, field: &str) {
    let error = evidence.validate(human).unwrap_err();
    assert_eq!(error.code(), code);
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["field"], field);
    assert!(wire.get("received").is_none());
    assert!(!wire.to_string().contains("SECRET_CONTENT"));
    assert!(!format!("{error:?}").contains("SECRET_CONTENT"));
    for example in wire["examples"].as_array().unwrap() {
        let value: serde_json::Value = serde_json::from_str(example.as_str().unwrap()).unwrap();
        let request: crate::runtime::api::Request = serde_json::from_value(value).unwrap();
        let crate::runtime::api::Request::Evidence(example) = request else {
            panic!()
        };
        example.validate(false).unwrap();
    }
}
#[test]
fn summaries_have_exact_byte_limits_and_reject_private_or_blank_text() {
    for text in ["é".repeat(4096), "x".repeat(8192), " exact\ntext ".into()] {
        let mut value = review();
        value.summary = text;
        value.validate(false).unwrap();
    }
    for text in ["é".repeat(4097), "x".repeat(8193)] {
        let mut value = review();
        value.summary = text;
        rejected(&value, true, "out_of_range", "/summary");
    }
    for text in [
        "",
        " \t\n",
        "\u{2003}",
        "SECRET_CONTENT\0",
        "<PRIVATE SECRET_CONTENT",
        "[private SECRET_CONTENT",
        "<!-- private SECRET_CONTENT",
        "-----BEGIN SECRET_CONTENT",
        "SECRET_CONTENT .comb/private",
        "SECRET_CONTENT AUTH.json",
        "SECRET_CONTENT credentials.json",
    ] {
        let mut value = review();
        value.summary = text.into();
        rejected(&value, true, "invalid_value", "/summary");
    }
}
#[test]
fn labels_are_bounded_relative_public_metadata_with_indexed_failures() {
    let mut value = review();
    value.files = vec!["é".repeat(256); 64];
    value.validate(false).unwrap();
    value.files.push("fine.rs".into());
    rejected(&value, true, "out_of_range", "/files");
    for text in ["x".repeat(513), "é".repeat(257)] {
        value.files = vec!["good.rs".into(), text];
        rejected(&value, true, "out_of_range", "/files/1");
    }
    for text in [
        "",
        "/SECRET_CONTENT",
        "~SECRET_CONTENT",
        "a\\SECRET_CONTENT",
        "a:SECRET_CONTENT",
        "a\nSECRET_CONTENT",
        "../SECRET_CONTENT",
        "a/./SECRET_CONTENT",
        "a//SECRET_CONTENT",
        "a/",
        ".",
        "..",
        "a/credentials.json",
        "a/.comb/private",
        "<PRIVATE_SECRET_CONTENT",
        "a\u{85}SECRET_CONTENT",
    ] {
        value.files = vec!["src/good.rs".into(), text.into(), "also/bad/".into()];
        rejected(&value, true, "invalid_value", "/files/1");
    }
    value.files = vec!["absent/repo/file.rs".into(), "..ordinary".into()];
    value.validate(false).unwrap();
}
#[test]
fn commit_and_verdict_rules_preserve_human_and_hosted_acceptance() {
    let mut value = review();
    for commit in ["abcdef0".into(), "ABCDEF0123456789".repeat(4)] {
        value.commit = Some(commit);
        value.validate(false).unwrap();
    }
    for commit in [
        String::new(),
        "abc123".into(),
        "f".repeat(65),
        "é".repeat(7),
        "SECRET_CONTENT".into(),
    ] {
        value.commit = Some(commit);
        rejected(&value, true, "invalid_value", "/commit");
    }
    value.commit = None;
    value.verdict = None;
    rejected(&value, false, "missing_argument", "/verdict");
    value.kind = EvidenceKind::Correction;
    value.validate(true).unwrap();
    rejected(&value, false, "invalid_value", "/kind");
    value.verdict = Some(Verdict::NoFindings);
    rejected(&value, true, "conflicting_arguments", "/verdict");
    // CLI and wire use the same spelling.
    assert_eq!(
        serde_json::to_value(Verdict::NoFindings).unwrap(),
        json!("no_findings")
    );
}

#[test]
fn verdict_cli_values_match_wire_values_without_aliases() {
    use clap::ValueEnum;
    for value in Verdict::value_variants() {
        let possible = value.to_possible_value().unwrap();
        assert_eq!(serde_json::to_value(value).unwrap(), possible.get_name());
        assert_eq!(possible.get_name_and_aliases().count(), 1);
    }
}
