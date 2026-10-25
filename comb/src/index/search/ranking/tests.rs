use super::*;

fn candidate(slug: &str, id: i64, route: Route) -> Candidate {
    Candidate {
        id,
        slug: slug.parse().unwrap(),
        score: 42.0,
        zone: Zone::Timeline,
        verified: false,
        title_match: false,
        exact_title: false,
        updated: None,
        route,
        evidence: if route == Route::Text {
            MatchEvidence::KeywordExact
        } else {
            MatchEvidence::SemanticSimilarity
        },
        safety: if route == Route::Text {
            CreateSafety::Probable
        } else {
            CreateSafety::Unknown
        },
    }
}

#[test]
fn rrf_preserves_zero_based_rank_bits_and_integer_rounding() {
    // Pinned values of the original 1.0 / (61.0 + rank as f64) expression.
    for (rank, bits) in [
        (0_u64, 0x3f90_c971_4fbc_da3b),
        (1, 0x3f90_8421_0842_1084),
        (59, 0x3f81_1111_1111_1111),
        (60, 0x3f80_ecf5_6be6_9c90),
        (1000, 0x3f4e_e252_84b4_7a92),
        (9_007_199_254_740_992, 0x3c9f_ffff_ffff_ffc4),
        (9_007_199_254_740_993, 0x3c9f_ffff_ffff_ffc4),
        (u64::MAX, 0x3bf0_0000_0000_0000),
    ] {
        if let Ok(rank) = usize::try_from(rank) {
            assert_eq!(rrf(rank).to_bits(), bits, "rank {rank}");
        }
    }
}

#[test]
fn equal_contributions_keep_the_text_chunk_and_add_in_route_order() {
    let mut text = candidate("concepts/shared", 10, Route::Text);
    text.zone = Zone::Compiled;
    text.verified = true;
    text.evidence = MatchEvidence::ExactTitle;
    text.safety = CreateSafety::Exists;
    let vector = candidate("concepts/shared", 20, Route::Vector);
    let merged = merge_hybrid(vec![text], vec![vector]);
    assert_eq!(merged.len(), 1);
    let hit = &merged[0];
    assert_eq!(hit.id, 10);
    assert_eq!(hit.zone, Zone::Compiled);
    assert!(hit.verified);
    assert_eq!(hit.route, Route::TextVector);
    assert_eq!(hit.evidence, MatchEvidence::ExactTitle);
    assert_eq!(hit.safety, CreateSafety::Exists);
    assert_eq!(hit.score.to_bits(), (rrf(0) + rrf(0)).to_bits());
}

#[test]
fn stronger_vector_selects_chunk_trust_but_keeps_text_identity() {
    let mut text = candidate("concepts/shared", 10, Route::Text);
    text.zone = Zone::Compiled;
    text.verified = true;
    text.evidence = MatchEvidence::Alias;
    text.safety = CreateSafety::Exists;
    let vector = candidate("concepts/shared", 20, Route::Vector);
    let merged = merge_hybrid(
        vec![candidate("concepts/first", 1, Route::Text), text],
        vec![vector],
    );
    let hit = merged.iter().find(|hit| hit.id == 20).unwrap();
    assert_eq!(hit.zone, Zone::Timeline);
    assert!(!hit.verified);
    assert_eq!(hit.route, Route::TextVector);
    assert_eq!(hit.evidence, MatchEvidence::Alias);
    assert_eq!(hit.safety, CreateSafety::Exists);
    assert_eq!(hit.score.to_bits(), (rrf(1) + rrf(0)).to_bits());
}

#[test]
fn merge_keeps_input_ranks_and_unmatched_routes_in_slug_order() {
    let merged = merge_hybrid(
        vec![
            candidate("concepts/shared", 10, Route::Text),
            candidate("concepts/a-text", 11, Route::Text),
        ],
        vec![
            candidate("concepts/z-vector", 20, Route::Vector),
            candidate("concepts/shared", 21, Route::Vector),
        ],
    );
    assert_eq!(
        merged.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [11, 10, 20]
    );
    assert_eq!(merged[0].route, Route::Text);
    assert_eq!(merged[0].score.to_bits(), rrf(1).to_bits());
    assert_eq!(merged[1].route, Route::TextVector);
    assert_eq!(merged[1].score.to_bits(), (rrf(0) + rrf(1)).to_bits());
    assert_eq!(merged[2].route, Route::Vector);
    assert_eq!(merged[2].evidence, MatchEvidence::SemanticSimilarity);
    assert_eq!(merged[2].safety, CreateSafety::Unknown);
    assert_eq!(merged[2].score.to_bits(), rrf(0).to_bits());
}

#[test]
fn bonuses_keep_verification_date_bypass_and_future_clamp() {
    let mut original = candidate("inbox/future", 1, Route::Text);
    original.score = rrf(0);
    original.zone = Zone::Compiled;
    original.title_match = true;
    original.updated = Some(i64::MAX);
    let mut unverified = original.clone();
    bonus(&mut unverified, Some(7.0), i64::MIN);
    assert_eq!(unverified.score.to_bits(), original.score.to_bits());
    original.verified = true;
    let mut filtered = original.clone();
    bonus(&mut filtered, None, i64::MIN);
    let mut expected = rrf(0);
    expected *= 1.1;
    expected *= 1.1;
    assert_eq!(filtered.score.to_bits(), expected.to_bits());
    bonus(&mut original, Some(7.0), i64::MIN);
    expected *= 1.0 + 0.1 * 7.0 / (7.0 + 0.0);
    assert_eq!(original.score.to_bits(), expected.to_bits());
}

#[test]
fn exact_slug_keeps_exact_evidence_without_a_title_bonus() {
    let mut page = crate::golden::page(crate::Actor::Human);
    page.metadata.authored.title = "Unrelated heading".into();
    page.compiled = "A body with no matching title.".into();
    page.timeline.clear();
    page.promote(crate::Actor::Human).unwrap();
    let slug = "concepts/exact-slug";
    let (_root, comb) = crate::disk::golden::seeded_comb(&[(slug, page)]);
    let request = crate::search::Search {
        query: slug.into(),
        ..crate::search::Search::default()
    };
    let index = comb.index().unwrap();
    let candidates = super::super::text_candidates(index, &request).unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].exact_title);
    assert!(!candidates[0].title_match);
    let result = index.search(&request).unwrap();
    let hit = &result.results[0];
    assert_eq!(hit.evidence, MatchEvidence::ExactTitle);
    assert_eq!(hit.create_safety, CreateSafety::Exists);
    assert_eq!(hit.title, "Unrelated heading");
    assert!(!hit.unverified);
    assert_eq!(hit.score.to_bits(), (rrf(0) * 1.1).to_bits());
}
