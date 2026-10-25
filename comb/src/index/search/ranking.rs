//! Pure page ranking over compact metadata, with one caller-supplied clock.
use crate::values::{CreateSafety, MatchEvidence, Route, Slug, Zone};
use std::collections::BTreeMap;

use crate::convert::{i64_f64, usize_f64};
use crate::index::limits::{
    COMPILED_MULTIPLIER, RECENCY_BONUS, RRF_K, SECONDS_PER_DAY, TITLE_MULTIPLIER,
};

// Rank only compact metadata. Corpus-sized text/title/heading material is never
// retained in candidates; hydrate final hits after both ranks and bonuses settle.
#[derive(Clone)]
pub(super) struct Candidate {
    pub(super) id: i64,
    pub(super) slug: Slug,
    pub(super) score: f64,
    pub(super) zone: Zone,
    pub(super) verified: bool,
    pub(super) title_match: bool,
    // SQL also marks exact slug matches, independently of title_match.
    pub(super) exact_title: bool,
    pub(super) updated: Option<i64>,
    pub(super) route: Route,
    pub(super) evidence: MatchEvidence,
    pub(super) safety: CreateSafety,
}
pub(super) fn order(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.score
        .total_cmp(&a.score)
        .then_with(|| a.slug.cmp(&b.slug))
}
pub(super) fn bonus(hit: &mut Candidate, half_life: Option<f64>, now: i64) {
    if !hit.verified {
        return;
    }
    if hit.zone == Zone::Compiled {
        hit.score *= COMPILED_MULTIPLIER;
    }
    if hit.title_match {
        hit.score *= TITLE_MULTIPLIER;
    }
    let Some(half_life) = half_life else {
        return;
    };
    if half_life == 0.0 {
        return;
    }
    if let Some(updated) = hit.updated {
        let days = i64_f64(now.saturating_sub(updated).max(0)) / SECONDS_PER_DAY;
        hit.score *= 1.0 + RECENCY_BONUS * half_life / (half_life + days);
    }
}

pub(super) fn rrf(rank: usize) -> f64 {
    // Keep the original 61.0 + rank evaluation, including integer-to-f64 rounding.
    1.0 / ((RRF_K + 1.0) + usize_f64(rank))
}

/// Sum route ranks in text-then-vector order. The larger contribution chooses
/// chunk hydration; equal contributions retain text. Text identity evidence and
/// create safety survive even when a vector chunk contributes the larger rank.
pub(super) fn merge_hybrid(text: Vec<Candidate>, vectors: Vec<Candidate>) -> Vec<Candidate> {
    let mut merged = BTreeMap::new();
    for (rank, mut hit) in text.into_iter().enumerate() {
        hit.score = rrf(rank);
        merged.insert(hit.slug.clone(), hit);
    }
    for (rank, mut hit) in vectors.into_iter().enumerate() {
        let contribution = rrf(rank);
        if let Some(prior) = merged.get_mut(&hit.slug) {
            let score = prior.score + contribution;
            let identity = (prior.safety, prior.evidence);
            if contribution > prior.score {
                *prior = hit;
            }
            prior.score = score;
            prior.route = Route::TextVector;
            prior.safety = identity.0;
            prior.evidence = identity.1;
        } else {
            hit.score = contribution;
            merged.insert(hit.slug.clone(), hit);
        }
    }
    merged.into_values().collect()
}

#[cfg(test)]
mod tests;
