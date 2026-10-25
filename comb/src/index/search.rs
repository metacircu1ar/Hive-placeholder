//! Filtered retrieval, ranking and final-hit hydration on the derived index.
use super::limits::{HEADING_CHARS, IDENTITY_LONG_QUERY_CHARS};
use crate::{
    Error, Result,
    embedding::Query,
    index::Index,
    search::{Hit, Mode, Results, Search, validate},
    values::{CreateSafety, MatchEvidence, Prefix, Route, Zone},
};
use std::collections::BTreeMap;

mod bindings;
mod ranking;
mod snippet;
mod sql;
mod vector;
pub(super) use bindings::bind;
use ranking::{Candidate, bonus, merge_hybrid, order, rrf};
use snippet::snippet;
pub(super) use sql::COVERAGE;

#[cfg(test)]
mod filter_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod recency_tests;

pub enum QueryVector {
    NotRequested,
    Vector(Query),
    Unavailable(Error),
}
impl From<Result<Query>> for QueryVector {
    fn from(query: Result<Query>) -> Self {
        match query {
            Ok(query) => Self::Vector(query),
            Err(error) => Self::Unavailable(error),
        }
    }
}

impl QueryVector {
    fn into_result(self) -> Result<Query> {
        match self {
            Self::NotRequested => Err(Error::query_model_missing()),
            Self::Vector(query) => Ok(query),
            Self::Unavailable(error) => Err(error),
        }
    }
}

fn candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        id: row.get(0)?,
        slug: row.get(1)?,
        zone: if row.get::<_, bool>(2)? {
            Zone::Compiled
        } else {
            Zone::Timeline
        },
        verified: row.get(3)?,
        title_match: row.get(4)?,
        updated: row.get(5)?,
        exact_title: row.get(6)?,
        score: 0.0,
        route: Route::Text,
        evidence: MatchEvidence::KeywordExact,
        safety: CreateSafety::Probable,
    })
}
fn text_candidates(index: &Index, request: &Search) -> Result<Vec<Candidate>> {
    validate(request)?;
    let lower = request.query.to_lowercase();
    let query = request
        .query
        .split_whitespace()
        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = index.connection.prepare_cached(sql::FTS)?;
    bind(
        &mut statement,
        request.filter(),
        &[(":query", &query), (":lower", &lower)],
    )?;
    let rows = statement.raw_query().mapped(candidate);
    let mut pages = BTreeMap::new();
    for item in rows {
        let mut hit = item?;
        if pages.contains_key(&hit.slug) {
            continue;
        }
        hit.score = rrf(pages.len());
        pages.insert(hit.slug.clone(), hit);
    }
    let identity = if request.query.chars().count() >= IDENTITY_LONG_QUERY_CHARS {
        sql::IDENTITY_LONG
    } else {
        sql::IDENTITY_SHORT
    };
    let identity_query = if request.query.chars().count() >= IDENTITY_LONG_QUERY_CHARS {
        format!("\"{}\"", lower.replace('"', "\"\""))
    } else {
        lower.clone()
    };
    let mut statement = index.connection.prepare_cached(identity)?;
    bind(
        &mut statement,
        request.filter(),
        &[(":query", &identity_query), (":lower", &lower)],
    )?;
    for (rank, item) in statement.raw_query().mapped(candidate).enumerate() {
        let mut hit = item?;
        hit.score = rrf(rank);
        hit.evidence = MatchEvidence::IdentitySubstring;
        pages.entry(hit.slug.clone()).or_insert(hit);
    }
    let mut statement = index.connection.prepare_cached(sql::EXACT)?;
    bind(
        &mut statement,
        request.filter(),
        &[(":query", &lower), (":lower", &lower)],
    )?;
    for item in statement.raw_query().mapped(candidate) {
        let mut hit = item?;
        hit.score = rrf(0);
        hit.evidence = if hit.exact_title {
            MatchEvidence::ExactTitle
        } else {
            MatchEvidence::Alias
        };
        hit.safety = CreateSafety::Exists;
        pages.insert(hit.slug.clone(), hit);
    }
    let mut values = pages.into_values().collect::<Vec<_>>();
    values.sort_by(order);
    Ok(values)
}
#[cfg(test)]
thread_local! { pub(crate) static HYDRATED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn finish(index: &Index, mut candidates: Vec<Candidate>, request: &Search) -> Result<Vec<Hit>> {
    let now = chrono::Utc::now().timestamp();
    for hit in &mut candidates {
        let half_life = (hit.verified && request.after.is_none() && request.before.is_none())
            .then(|| Prefix::half_life_days(&hit.slug));
        bonus(hit, half_life, now);
    }
    candidates.sort_by(order);
    candidates.truncate(request.limit);
    let mut statement = index.connection.prepare_cached(sql::HYDRATE)?;
    candidates
        .into_iter()
        .map(|c| {
            #[cfg(test)]
            HYDRATED.set(HYDRATED.get() + 1);
            let mut hit = statement.query_row([c.id], |r| {
                Ok(Hit {
                    slug: c.slug,
                    title: r.get(0)?,
                    kind: r.get(1)?,
                    snippet: r.get(2)?,
                    heading: r.get(3)?,
                    zone: r.get(4)?,
                    unverified: !r.get::<_, bool>(5)?,
                    page_unverified: !r.get::<_, bool>(6)?,
                    score: c.score,
                    matched_by: c.route,
                    evidence: c.evidence,
                    create_safety: c.safety,
                    highlights: Vec::new(),
                })
            })?;
            let snippet = snippet(&hit.snippet, &request.query);
            hit.snippet = snippet.text;
            hit.highlights = snippet.highlights;
            // The full path remains in get; bound search presentation even for a
            // pathological megabyte-long Markdown heading.
            if hit.heading.chars().count() > HEADING_CHARS {
                hit.heading = hit
                    .heading
                    .chars()
                    .take(HEADING_CHARS)
                    .chain(std::iter::once('…'))
                    .collect();
            }
            Ok(hit)
        })
        .collect()
}
impl Index {
    pub fn search(&self, request: &Search) -> Result<Results> {
        Ok(Results {
            results: finish(self, text_candidates(self, request)?, request)?,
            mode: Mode::Text,
            fallback: None,
            coverage: None,
        })
    }
    pub fn retrieve(&self, request: &Search, mode: Mode, query: QueryVector) -> Result<Results> {
        validate(request)?;
        if mode == Mode::Text {
            return self.search(request);
        }
        match query
            .into_result()
            .and_then(|q| self.vector_candidates(request, &q))
        {
            Err(error)
                if mode == Mode::Hybrid && error.code != crate::ErrorCode::RecoveryRequired =>
            {
                let mut results = self.search(request)?;
                results.fallback = Some(error.code);
                results.coverage = Some(self.embedding_coverage(request.filter())?);
                tracing::debug!(name: "comb.search_fallback",
                     code = error.code.as_str(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "text_results");
                Ok(results)
            }
            Err(error) => Err(error),
            Ok((mut vectors, coverage)) => {
                if mode == Mode::Vector {
                    for (rank, hit) in vectors.iter_mut().enumerate() {
                        hit.score = rrf(rank);
                    }
                    return Ok(Results {
                        results: finish(self, vectors, request)?,
                        mode: Mode::Vector,
                        fallback: None,
                        coverage: Some(coverage),
                    });
                }
                let merged = merge_hybrid(text_candidates(self, request)?, vectors);
                Ok(Results {
                    results: finish(self, merged, request)?,
                    mode: Mode::Hybrid,
                    fallback: None,
                    coverage: Some(coverage),
                })
            }
        }
    }
}
