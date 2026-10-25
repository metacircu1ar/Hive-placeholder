use super::*;
use crate::runtime::api::value;

#[test]
fn evidence_preserves_raw_payloads_and_explicit_path_bytes() {
    let record = serde_json::json!({"foreign":[null,true,u64::MAX]});
    assert_eq!(
        value(Evidence {
            kind: "ownership_evidence",
            path: "raw".into(),
            path_bytes: Some(&[0, 255]),
            outcome: EvidenceOutcome::Record { record },
        })
        .to_string(),
        r#"{"path":"raw","path_bytes":[0,255],"record":{"foreign":[null,true,18446744073709551615]},"type":"ownership_evidence"}"#
    );
    assert_eq!(
        value(Evidence {
            kind: "ownership_evidence",
            path: "missing".into(),
            path_bytes: None,
            outcome: EvidenceOutcome::Unavailable {
                unavailable: "state_io: unavailable".into()
            },
        })
        .to_string(),
        r#"{"path":"missing","path_bytes":null,"type":"ownership_evidence","unavailable":"state_io: unavailable"}"#
    );
}

#[test]
fn manifest_report_and_blob_bytes_remain_exact() {
    crate::compatibility::contract(
        "controller-export.json",
        &Manifest {
            kind: "manifest",
            source: Path::new("/retired"),
            consistent_snapshot: true,
            format: hive_input::compatibility::Exact,
            encoding: "ndjson",
        },
    );
    assert_eq!(
        value(Manifest {
            kind: "manifest",
            source: Path::new("/retired"),
            consistent_snapshot: true,
            format: hive_input::compatibility::Exact,
            encoding: "ndjson",
        })
        .to_string(),
        r#"{"consistent_snapshot":true,"encoding":"ndjson","format":1,"source":"/retired","type":"manifest"}"#
    );
    assert_eq!(
        value(Report {
            operation: "history_export",
            source: Path::new("/retired"),
            output: Path::new("/export"),
            rows: 3,
            ownership_evidence: EvidenceSummary {
                records: 1,
                unavailable: 2,
                consistency: "each record checked independently; not atomic with database",
            },
            large_output_files: "retained in the original root; not included",
        })
        .to_string(),
        r#"{"large_output_files":"retained in the original root; not included","operation":"history_export","output":"/export","ownership_evidence":{"consistency":"each record checked independently; not atomic with database","records":1,"unavailable":2},"rows":3,"source":"/retired"}"#
    );
    let row = Map::from_iter([(
        "raw".into(),
        value(Blob {
            encoding: "hex",
            data: "00ff".into(),
        }),
    )]);
    assert_eq!(
        value(Row {
            table: "events",
            row
        })
        .to_string(),
        r#"{"row":{"raw":{"data":"00ff","encoding":"hex"}},"table":"events"}"#
    );
}
