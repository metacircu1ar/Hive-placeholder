//! SQLite produces vector candidates; each row carries exactly one vector source.
use super::ranking::Candidate;
use super::{bind, candidate, order, sql};
use crate::{
    Error, Result,
    embedding::{self, Coverage, Query},
    index::{Index, cache::Resident},
    search::Search,
    values::{ContentHash, CreateSafety, MatchEvidence, Route, Slug},
};
use std::{collections::BTreeMap, sync::Arc};

enum VectorSource<'a> {
    Resident(&'a Resident, ContentHash),
    Inline(Vec<u8>),
}
impl VectorSource<'_> {
    fn load(self, index: &Index, dimensions: usize) -> Result<Arc<[f32]>> {
        match self {
            Self::Resident(cache, hash) => index.cached_vector(cache, &hash),
            Self::Inline(bytes) => Ok(embedding::decode(&bytes, dimensions)?.into()),
        }
    }
}

impl Index {
    pub(super) fn vector_candidates(
        &self,
        request: &Search,
        query: &Query,
    ) -> Result<(Vec<Candidate>, Coverage)> {
        self.check_lease(&query.lease)?;
        let coverage = self.embedding_coverage(request.filter())?;
        if coverage.embedded == 0 && coverage.total > 0 {
            return Err(Error::filtered_vectors_missing());
        }
        let resident = self
            .vectors
            .as_ref()
            .filter(|cache| cache.matches(&query.lease));
        let sql = if resident.is_some() {
            sql::VECTOR_HASH
        } else {
            sql::VECTOR_BLOB
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        bind(
            &mut statement,
            request.filter(),
            &[
                (":fingerprint", &query.lease.fingerprint),
                (":lower", &request.query.to_lowercase()),
            ],
        )?;
        let rows = statement.raw_query().mapped(|row| {
            Ok((
                candidate(row)?,
                match resident {
                    Some(cache) => VectorSource::Resident(cache, row.get(7)?),
                    None => VectorSource::Inline(row.get(7)?),
                },
            ))
        });
        let mut pages: BTreeMap<Slug, Candidate> = BTreeMap::new();
        for item in rows {
            let (mut hit, source) = item?;
            let vector = source.load(self, query.lease.dimensions)?;
            hit.score = vector
                .iter()
                .zip(&query.vector)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            hit.route = Route::Vector;
            hit.evidence = MatchEvidence::SemanticSimilarity;
            hit.safety = CreateSafety::Unknown;
            match pages.entry(hit.slug.clone()) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(hit);
                }
                std::collections::btree_map::Entry::Occupied(mut e)
                    if hit.score > e.get().score =>
                {
                    e.insert(hit);
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
        }
        let mut values = pages.into_values().collect::<Vec<_>>();
        values.sort_by(order);
        Ok((values, coverage))
    }
}
