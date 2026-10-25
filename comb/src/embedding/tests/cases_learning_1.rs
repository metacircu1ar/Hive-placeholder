//! Learning regressions.
use super::*;

#[test]
fn partial_coverage_preserves_semantic_matches_and_reports_rejections_and_pending() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    put(&mut comb, "concepts/huge", "Huge", "OVERSIZED");
    fill(&mut comb, &mut mock);
    let q = query(&comb, &mut mock, "kitten");
    let result = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(result.mode, Mode::Hybrid);
    assert_eq!(result.results[0].slug, "concepts/cat".parse().unwrap());
    assert_eq!(result.results[0].create_safety, CreateSafety::Unknown);
    assert_eq!(
        result.coverage.unwrap(),
        Coverage {
            total: 2,
            embedded: 1,
            rejected: 1,
            pending: 0
        }
    );
    assert!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::EmbeddingRejected)
    );
    put(&mut comb, "concepts/new", "New", "feline pending");
    let q = query(&comb, &mut mock, "kitten");
    let result = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(result.mode, Mode::Hybrid);
    assert_eq!(result.coverage.unwrap().pending, 1);
    fill(&mut comb, &mut mock);
    assert_eq!(coverage(&comb).embedded, 2);
    comb.retry_embedding_failures().unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .embedding_jobs(&mut Scan::default(), 32)
            .unwrap()
            .jobs
            .len(),
        1
    );
}
