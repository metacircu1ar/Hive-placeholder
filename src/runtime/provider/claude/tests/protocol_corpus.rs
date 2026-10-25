//! Synthetic Claude boundary fixtures; native raw evidence is not available in the repository.
use super::*;
use serde::Deserialize;

const SESSION: &str = "00000000-0000-0000-0000-000000000001";
#[derive(Deserialize)]
struct Case {
    name: String,
    kind: String,
    provenance: String,
    raw: String,
    metadata: Option<bool>,
    #[serde(default)]
    notices: Vec<usize>,
    #[serde(default)]
    error: bool,
}
fn cases(kind: &str) -> Vec<Case> {
    #[derive(Deserialize)]
    struct Corpus {
        provider: String,
        provider_version: String,
        provenance: String,
        cases: Vec<Case>,
    }
    let corpus: Corpus = serde_json::from_str(include_str!(
        "../../../../provider_support/corpora/claude-2.1.285.json"
    ))
    .unwrap();
    assert_eq!(corpus.provider, "claude");
    assert_eq!(
        crate::provider_support::registry(crate::config::Provider::Claude).corpus_versions,
        [corpus.provider_version.as_str()]
    );
    assert!(!corpus.provenance.is_empty());
    let cases = corpus.cases;
    cases.into_iter().filter(|case| case.kind == kind).collect()
}

#[test]
fn preinput_corpus_preserves_execution_evidence_classification() {
    for case in cases("preinput") {
        assert!(!case.provenance.is_empty());
        let mut fixture = Fixture::with_id(None, SESSION.into());
        let raw: Value = serde_json::from_str(&case.raw).unwrap();
        let _ = fixture.codec.receive(raw);
        fixture.codec.guard.seal();
        assert_eq!(
            fixture.codec.preinput_quiescent(),
            case.metadata.unwrap(),
            "{}",
            case.name
        );
    }
}

#[test]
fn user_corpus_forwards_whole_tool_results_and_keeps_prefix_effects() {
    for case in cases("user") {
        assert!(!case.provenance.is_empty());
        let mut fixture = Fixture::with_id(None, SESSION.into());
        fixture.turn();
        let raw: Value = serde_json::from_str(&case.raw).unwrap();
        let result = fixture.codec.receive(raw.clone());
        assert_eq!(
            result.as_ref().err().map(crate::error::Error::code),
            case.error.then_some("provider_protocol_error"),
            "{}",
            case.name
        );
        for index in case.notices {
            let frame = fixture.codec.pop().unwrap();
            assert_eq!(frame["method"], "item/completed", "{}", case.name);
            let block = &raw["message"]["content"][index];
            assert_eq!(frame["params"]["item"]["result"], *block, "{}", case.name);
            assert_eq!(
                serde_json::to_vec(&frame["params"]["item"]["result"]).unwrap(),
                serde_json::to_vec(block).unwrap(),
                "{}",
                case.name
            );
        }
        assert!(fixture.codec.pop().is_none(), "{}", case.name);
    }
}
