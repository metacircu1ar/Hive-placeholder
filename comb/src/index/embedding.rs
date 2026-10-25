//! Embedding selection, leases and coverage on the derived index.
use crate::convert::usize_u32_capped;
use crate::{
    Error, Result,
    embedding::{BATCH_SIZE, Batch, Coverage, Job, Lease, Scan, Space},
    search::Filter,
    values::{ActivationId, Fingerprint},
};
use rusqlite::{OptionalExtension, params};

const JOBS: &str = "SELECT c.id,c.instance,c.input_hash,\
p.title||char(10)||c.heading||char(10)||c.text,c.slug,c.ordinal FROM chunks c JOIN pages p ON \
p.slug=c.slug\n             WHERE c.id>?3 AND NOT EXISTS(SELECT 1 FROM vector_cache.vectors v \
WHERE v.fingerprint=?1 AND v.input_hash=c.input_hash)\n             AND NOT EXISTS(SELECT 1 \
FROM embedding_failures f WHERE f.chunk_id=c.id AND f.fingerprint=?1)\n             ORDER BY \
c.id LIMIT ?2";
const ACTIVE_SPACE: &str =
    "SELECT fingerprint,activation,specification FROM embedding_space WHERE singleton=1";

impl crate::index::Index {
    pub fn embedding_jobs(&self, scan: &mut Scan, limit: usize) -> Result<Batch> {
        if !(1..=BATCH_SIZE).contains(&limit) {
            return Err(Error::embedding_batch_limit());
        }
        let (lease, _) = self
            .active_space()?
            .ok_or_else(Error::active_space_missing)?;
        if scan.activation.as_ref() != Some(&lease.activation) {
            scan.activation = Some(lease.activation.clone());
            scan.after = 0;
        }
        // The validated batch bound fits u32; conversion stays in the binding expression.
        let mut statement = self.connection.prepare(JOBS)?;
        let jobs: Vec<_> = statement
            .query_map(
                params![lease.fingerprint, usize_u32_capped(limit), scan.after],
                |r| {
                    Ok(Job {
                        id: r.get(0)?,
                        instance: r.get(1)?,
                        input_hash: r.get(2)?,
                        input: r.get(3)?,
                        slug: r.get(4)?,
                        ordinal: r.get(5)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<_>>()?;
        scan.after = jobs.last().map_or(0, |job| job.id);
        #[cfg(test)]
        {
            let raw_steps = statement.get_status(rusqlite::StatementStatus::VmStep);
            let Ok(steps) = u64::try_from(raw_steps) else {
                panic!("negative SQLite VM-step count: {raw_steps}");
            };
            scan.steps += steps;
        }
        Ok(Batch { lease, jobs })
    }
    pub(crate) fn active_space(&self) -> Result<Option<(Lease, Space)>> {
        let row: Option<(String, String, String)> = self
            .connection
            .query_row(ACTIVE_SPACE, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        row.map(|(fingerprint, activation, json)| {
            let space: Space = serde_json::from_str(&json)?;
            space.validate()?;
            if space.fingerprint()?.as_str() != fingerprint {
                return Err(Error::invalid_active_space());
            }
            let fingerprint = Fingerprint::try_from(fingerprint)
                .map_err(|cause| Error::invalid_active_space().caused_by(cause))?;
            let activation = ActivationId::try_from(activation)?;
            Ok((
                Lease {
                    fingerprint,
                    activation,
                    dimensions: space.dimensions,
                },
                space,
            ))
        })
        .transpose()
    }
    pub fn embedding_lease(&self, space: &Space) -> Result<Lease> {
        let (lease, _) = self.active_space()?.ok_or_else(Error::embed_first)?;
        if lease.fingerprint != space.fingerprint()? {
            return Err(Error::query_space_mismatch());
        }
        Ok(lease)
    }
    pub(crate) fn check_lease(&self, expected: &Lease) -> Result<()> {
        match self.active_space()? {
            Some((lease, _)) if &lease == expected => Ok(()),
            _ => Err(Error::embedding_activation_changed()),
        }
    }
    pub fn embedding_coverage(&self, filter: Filter<'_>) -> Result<Coverage> {
        filter.validate()?;
        let fingerprint = self.active_space()?.map(|(lease, _)| lease.fingerprint);
        let mut statement = self.connection.prepare_cached(super::search::COVERAGE)?;
        super::search::bind(&mut statement, filter, &[(":fingerprint", &fingerprint)])?;
        Ok(statement
            .raw_query()
            .mapped(|r| coverage(r.get(0)?, r.get(1)?, r.get(2)?))
            .next()
            .ok_or(rusqlite::Error::QueryReturnedNoRows)??)
    }
}

fn coverage(total: i64, embedded: i64, rejected: i64) -> rusqlite::Result<Coverage> {
    let total = count(total, 0)?;
    let embedded = count(embedded, 1)?;
    let rejected = count(rejected, 2)?;
    let pending = total
        .checked_sub(embedded)
        .and_then(|value| value.checked_sub(rejected))
        .ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::other("Coverage counts exceed their total.")),
            )
        })?;
    Ok(Coverage {
        total,
        embedded,
        pending,
        rejected,
    })
}
fn count(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|cause| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(cause),
        )
    })
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn coverage_preserves_zero_and_maximum_signed_counts() {
        let zero = coverage(0, 0, 0).unwrap();
        assert_eq!(
            (zero.total, zero.embedded, zero.pending, zero.rejected),
            (0, 0, 0, 0)
        );
        let max = u64::try_from(i64::MAX).unwrap();
        for (embedded, rejected, pending) in [(i64::MAX, 0, 0), (0, i64::MAX, 0), (0, 0, max)] {
            let result = coverage(i64::MAX, embedded, rejected).unwrap();
            assert_eq!(result.total, max);
            assert_eq!(result.pending, pending);
            assert_eq!(result.embedded, u64::try_from(embedded).unwrap());
            assert_eq!(result.rejected, u64::try_from(rejected).unwrap());
        }
    }
    #[test]
    fn corrupt_counts_use_the_existing_database_error_boundary() {
        for (total, embedded, rejected) in [
            (-1, 0, 0),
            (0, -1, 0),
            (0, 0, -1),
            (1, 2, 0),
            (2, 1, 2),
            (i64::MAX, i64::MAX, 1),
        ] {
            let Err(cause) = coverage(total, embedded, rejected) else {
                panic!("invalid counts accepted");
            };
            assert!(matches!(
                cause,
                rusqlite::Error::FromSqlConversionFailure(..)
            ));
            let error = Error::from(cause);
            assert_eq!(error.code, crate::ErrorCode::IndexError);
            assert!(!error.file_committed);
        }
    }
}
