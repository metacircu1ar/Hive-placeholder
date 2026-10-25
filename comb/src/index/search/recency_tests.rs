use super::{Candidate, Search, bonus};
use crate::{
    golden,
    values::{CreateSafety, MatchEvidence, Prefix, Route, Zone},
};
use serde::Serialize;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

#[derive(Serialize)]
struct Case {
    prefix: &'static str,
    age_days: i64,
    filter: &'static str,
    verified: bool,
    score_bits: u64,
}

fn score(prefix: &str, age_days: i64, filter: &str, verified: bool) -> f64 {
    let mut candidate = Candidate {
        id: 1,
        slug: format!("{prefix}/recency").parse().unwrap(),
        score: 1.0 / 61.0,
        zone: Zone::Compiled,
        verified,
        title_match: true,
        exact_title: true,
        updated: Some(NOW - age_days * DAY),
        route: Route::Text,
        evidence: MatchEvidence::ExactTitle,
        safety: CreateSafety::Exists,
    };
    let request = Search {
        query: "recency".into(),
        after: (filter == "after").then(|| "2020-01-01T00:00:00Z".parse().unwrap()),
        before: (filter == "before").then(|| "2040-01-01T00:00:00Z".parse().unwrap()),
        ..Search::default()
    };
    let half_life = (request.after.is_none() && request.before.is_none())
        .then(|| Prefix::half_life_days(&candidate.slug));
    bonus(&mut candidate, half_life, NOW);
    candidate.score
}

#[test]
fn recency_scores_match_the_fixed_clock_contract_bit_for_bit() {
    let cases: Vec<_> = [
        "inbox",
        "reports",
        "components",
        "concepts",
        "decisions",
        "operations",
    ]
    .into_iter()
    .flat_map(|prefix| {
        [-1, 0, 7, 90].into_iter().flat_map(move |age_days| {
            ["none", "after", "before"]
                .into_iter()
                .flat_map(move |filter| {
                    [true, false].into_iter().map(move |verified| Case {
                        prefix,
                        age_days,
                        filter,
                        verified,
                        score_bits: score(prefix, age_days, filter, verified).to_bits(),
                    })
                })
        })
    })
    .collect();
    golden::json("search-recency-bits.json", &cases);
}

#[test]
fn recency_prefix_policy_covers_bare_names_and_unknown_prefixes() {
    for (name, half_life) in [
        ("components", 90.0),
        ("decisions", 0.0),
        ("incidents", 90.0),
        ("concepts", 0.0),
        ("people", 90.0),
        ("operations", 0.0),
        ("inbox", 7.0),
        ("review", 90.0),
        ("reports", 7.0),
        ("custom", 90.0),
        ("inboxish", 90.0),
    ] {
        for slug in [name.to_owned(), format!("{name}/child")] {
            let mut hit = Candidate {
                id: 1,
                slug: slug.parse().unwrap(),
                score: 1.0 / 61.0,
                zone: Zone::Compiled,
                verified: true,
                title_match: true,
                exact_title: true,
                updated: Some(NOW - 7 * DAY),
                route: Route::Text,
                evidence: MatchEvidence::ExactTitle,
                safety: CreateSafety::Exists,
            };
            let policy = Prefix::half_life_days(&hit.slug);
            bonus(&mut hit, Some(policy), NOW);
            let mut expected: f64 = 1.0 / 61.0;
            expected *= 1.1;
            expected *= 1.1;
            if half_life != 0.0 {
                expected *= 1.0 + 0.1 * half_life / (half_life + 7.0);
            }
            assert_eq!(hit.score.to_bits(), expected.to_bits(), "{slug}");
        }
    }
}

#[test]
fn search_sql_compiled_literal_matches_typed_zone_bindings() {
    let (_root, comb) =
        crate::disk::golden::seeded_comb(&[("concepts/source", golden::page(crate::Actor::Human))]);
    let mut statement = comb
        .index()
        .unwrap()
        .connection
        .prepare(concat!(
            "SELECT ",
            super::sql::columns!(),
            ",c.zone FROM chunks c JOIN pages p ON p.slug=c.slug ORDER BY c.ordinal"
        ))
        .unwrap();
    let rows: Vec<_> = statement
        .query_map(rusqlite::named_params! {":lower": "source"}, |row| {
            Ok((
                super::candidate(row)?.zone,
                row.get::<_, crate::values::Zone>(7)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(rows[0], (Zone::Compiled, Zone::Compiled));
    assert_eq!(rows.len(), 6);
    assert!(
        rows[1..]
            .iter()
            .all(|row| *row == (Zone::Timeline, Zone::Timeline))
    );
}
