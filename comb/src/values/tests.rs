use super::*;

#[test]
fn identifiers_preserve_spelling_and_current_bounds() {
    let uuid = "ABCDEFAB-CDEF-4ABC-8ABC-ABCDEFABCDEF";
    let evidence: EvidenceId = uuid.parse().unwrap();
    assert_eq!(evidence.as_str(), uuid);
    assert_eq!(
        crate::error::json_text(&evidence).unwrap(),
        format!("\"{uuid}\"")
    );
    let comb: CombId = uuid.parse().unwrap();
    assert_eq!(comb.as_str(), uuid.to_ascii_lowercase());
    assert!("\0\n".parse::<RequestId>().is_err());
    assert!("".parse::<RequestId>().is_err());
    assert!("x".repeat(129).parse::<RequestId>().is_err());
    assert!("arbitrary-prefix/page".parse::<Slug>().is_ok());
    assert!("A/page".parse::<Slug>().is_err());
    assert!("a//page".parse::<Slug>().is_err());
    assert!("human".parse::<CredentialId>().unwrap().is_human());
    assert!(CredentialId::issued("human".into()).is_err());
    assert!("2026-01-02T03:04:05.123+04:00".parse::<Timestamp>().is_ok());
}

#[test]
fn principal_checks_fields_omitted_from_its_encoding() {
    let actor = Actor::Agent {
        bridge: "key".into(),
        id: "agent".into(),
        role: Some("role".into()),
        execution: "execution".into(),
        turn: "turn".into(),
    };
    assert_eq!(
        Principal::for_actor(&actor).unwrap().as_str(),
        "[\"agent\",\"key\",\"agent\"]"
    );
    let Actor::Agent {
        bridge,
        id,
        role,
        execution,
        ..
    } = actor
    else {
        unreachable!()
    };
    let invalid = Actor::Agent {
        bridge,
        id,
        role,
        execution,
        turn: String::new(),
    };
    assert_eq!(
        Principal::for_actor(&invalid).unwrap_err().code,
        crate::ErrorCode::InvalidActor
    );
}

#[test]
fn values_serialize_as_strings_and_bind_sql_by_borrow() {
    use rusqlite::types::{ToSql, ToSqlOutput, ValueRef};
    macro_rules! check {
        ($kind:ty, $text:expr) => {{
            let text = ($text).to_owned();
            let value: $kind = text.parse().unwrap();
            let encoded = crate::error::json_text(&value).unwrap();
            assert_eq!(encoded, crate::error::json_text(&text).unwrap());
            assert_eq!(serde_json::from_str::<$kind>(&encoded).unwrap(), value);
            assert!(matches!(value.to_sql().unwrap(), ToSqlOutput::Borrowed(ValueRef::Text(bytes)) if bytes == text.as_bytes()));
        }};
    }
    check!(Slug, "custom-name/part_2");
    check!(ContentHash, "ab".repeat(32).as_str());
    check!(Fingerprint, "cd".repeat(32).as_str());
    check!(EvidenceId, "urn:uuid:ABCDEFAB-CDEF-4ABC-8ABC-ABCDEFABCDEF");
    check!(ActivationId, "{ABCDEFAB-CDEF-4ABC-8ABC-ABCDEFABCDEF}");
    check!(CredentialId, "ABCDEFABCDEF4ABC8ABCABCDEFABCDEF");
    check!(CredentialId, "human");
    check!(CombId, "abcdefab-cdef-4abc-8abc-abcdefabcdef");
    check!(Timestamp, "2026-01-02T03:04:05.123+04:00");
    check!(RequestId, "request α");
    check!(RunName, "run α");
    check!(Principal, "[\"agent\",\"key\",\"arbitrary actor\"]");
    assert!("AB".repeat(32).parse::<ContentHash>().is_err());
    assert!("ab".repeat(31).parse::<Fingerprint>().is_err());
    assert!("\n".parse::<RunName>().is_err());
    assert!("é".repeat(256).parse::<RunName>().is_ok());
    assert!("é".repeat(257).parse::<RunName>().is_err());
    assert!("x".repeat(128).parse::<RequestId>().is_ok());
    assert!("human ".parse::<CredentialId>().is_err());
}

#[test]
fn metadata_dates_keep_empty_wire_values_and_contextual_errors() {
    for fields in ["", "created_at: ''\nupdated_at: ''\n"] {
        let page = crate::Page::parse(&format!("---\ntitle: Dates\n{fields}---\nText")).unwrap();
        assert!(page.metadata.owned().created_at().is_none());
        assert!(page.metadata.owned().updated_at().is_none());
        let encoded = crate::error::value(&page.metadata).unwrap();
        assert_eq!(encoded["created_at"], "");
        assert_eq!(encoded["updated_at"], "");
    }
    for field in ["created_at", "updated_at"] {
        for value in ["' '", "not-a-date"] {
            let error =
                crate::Page::parse(&format!("---\ntitle: Dates\n{field}: {value}\n---\nText"))
                    .unwrap_err();
            assert_eq!(error.code, crate::ErrorCode::InvalidPage);
            assert_eq!(error.message, Error::invalid_page_timestamps().message);
        }
        let error = crate::Page::parse(&format!("---\ntitle: Dates\n{field}: null\n---\nText"))
            .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::InvalidPage);
    }
    let at = "2026-01-02T03:04:05.123+04:00";
    let page = crate::Page::parse(&format!(
        "---\ntitle: Dates\ncreated_at: '{at}'\nupdated_at: '{at}'\n---\nText"
    ))
    .unwrap();
    assert_eq!(page.metadata.owned().created_at().unwrap().as_str(), at);
    assert_eq!(page.metadata.owned().updated_at().unwrap().as_str(), at);
}

#[test]
fn raw_evidence_checks_uuid_then_timestamp_then_source() {
    let entry = |id: &str, at: &str, source: &str| {
        let json = serde_json::json!({"id":id,"at":at,"source":source,"actor":{"kind":"external"},"run":null});
        format!("<!-- comb-entry {json} -->\nEvidence\n")
    };
    let page = |timeline: String| {
        crate::Page::parse(&format!(
            "---\ntitle: Evidence\n---\nSummary\n\n---\n## Timeline\n\n{timeline}"
        ))
    };
    let id = "abcdefab-cdef-4abc-8abc-abcdefabcdef";
    let error = page(entry("invalid", "invalid", "")).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::InvalidTimeline);
    assert_eq!(error.message, Error::invalid_evidence_id().message);
    let error = page(entry(id, "invalid", "")).unwrap_err();
    assert_eq!(error.message, Error::invalid_evidence_timestamp().message);
    let error = page(entry(id, "2026-01-02T03:04:05Z", "")).unwrap_err();
    assert_eq!(error.message, Error::invalid_evidence_source().message);
    let lower = entry(id, "2026-01-02T03:04:05Z", "fixture");
    let upper = entry(&id.to_uppercase(), "2026-01-02T03:04:05Z", "fixture");
    let parsed = page(format!("{lower}\n{upper}")).unwrap();
    assert_eq!(parsed.timeline.len(), 2);
    assert_ne!(
        parsed.timeline[0].metadata.authored.id,
        parsed.timeline[1].metadata.authored.id
    );
    assert_eq!(
        page(format!("{lower}\n{lower}")).unwrap_err().message,
        Error::duplicate_evidence_id().message
    );
}

#[test]
fn untrusted_metadata_hints_and_direct_run_attribution_remain_raw() {
    let mut page = crate::golden::page(Actor::Human);
    crate::golden::edit_record(&mut page, |raw| {
        raw["metadata"]["promotion"]["digest"] = serde_json::json!("not-a-digest");
        raw["metadata"]["promotion"]["at"] = serde_json::json!("not-a-date");
        raw["timeline"][0]["metadata"]["run"] = serde_json::json!("\nold attribution");
        raw["timeline"][0]["metadata"]["actor"] = serde_json::json!(Actor::Client {
            id: String::new(),
            name: String::new(),
        });
        raw["timeline"][0]["metadata"]["consolidation"]["source"] =
            serde_json::json!("../untrusted");
        raw["timeline"][0]["metadata"]["consolidation"]["source_hash"] =
            serde_json::json!("not-a-hash");
    });
    let rendered = page.render().unwrap();
    let parsed = crate::Page::parse(&rendered).unwrap();
    assert_eq!(parsed.render().unwrap(), rendered);
    let request = "raw-run".parse().unwrap();
    assert!(crate::store::Write::new(&Actor::Human, &request, Some("\n")).is_ok());
    assert!("\n".parse::<RunName>().is_err());
}
