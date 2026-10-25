use super::{Backfill, BackfillHost, BatchSizer};
use crate::{
    Actor, Comb, Error, ErrorCode, Result,
    embedding::{Batch, Embedder, Job, Lease, Scan, Space},
    locks::Health,
    search::Filter,
    store::Write,
    values::{ActivationId, ContentHash, Fingerprint, RequestId},
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::Duration,
};

#[test]
fn sizing_starts_at_one_and_clamps_short_and_long_samples() {
    assert_eq!(BatchSizer::default().limit(), 1);
    for (elapsed, expected) in [
        (Duration::ZERO, 32),
        (Duration::from_nanos(1), 32),
        (Duration::from_micros(6250), 32),
        (Duration::from_nanos(6_250_001), 31),
        (Duration::from_millis(10), 20),
        (Duration::from_millis(100), 2),
        (Duration::from_millis(200), 1),
        (Duration::from_secs(3), 1),
    ] {
        let mut sizer = BatchSizer::default();
        sizer.record(1, elapsed);
        assert_eq!(sizer.limit(), expected, "sample {elapsed:?}");
    }
}

#[test]
fn successful_samples_keep_exact_per_input_ewma_bits_and_next_limits() {
    let mut sizer = BatchSizer::default();
    for (inputs, elapsed, bits, limit) in [
        (1, 10, 0x3f84_7ae1_47ae_147b, 20),
        (2, 60, 0x3f8e_b851_eb85_1eb8, 13),
        (1, 100, 0x3fa2_8f5c_28f5_c290, 5),
        (4, 1000, 0x3fb6_f5c2_8f5c_28f6, 2),
    ] {
        sizer.record(inputs, Duration::from_millis(elapsed));
        assert_eq!(sizer.per_input.map(f64::to_bits), Some(bits));
        assert_eq!(sizer.limit(), limit);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Select { after: i64, limit: usize },
    Process(Vec<i64>),
}
struct Host {
    batch: Batch,
    trace: Vec<Event>,
    selection_error: Option<Error>,
    progress: Result<Option<&'static str>>,
    panic_on_process: bool,
}
impl BackfillHost for Host {
    type Progress = &'static str;
    fn select(&mut self, scan: &mut Scan, limit: usize) -> Result<Batch> {
        self.trace.push(Event::Select {
            after: scan.after,
            limit,
        });
        scan.activation = Some(self.batch.lease.activation.clone());
        scan.after = self.batch.jobs.last().map_or(0, |job| job.id);
        scan.steps += 1;
        if let Some(error) = self.selection_error.take() {
            return Err(error);
        }
        Ok(self.batch.clone())
    }
    fn process(&mut self, batch: Batch) -> Result<Option<&'static str>> {
        self.trace.push(Event::Process(
            batch.jobs.iter().map(|job| job.id).collect(),
        ));
        assert!(!self.panic_on_process, "inference panic");
        std::mem::replace(&mut self.progress, Ok(None))
    }
}
fn host(ids: &[i64]) -> Result<Host> {
    let mut jobs = Vec::new();
    for &id in ids {
        jobs.push(Job {
            slug: "concepts/page".parse()?,
            ordinal: 0,
            id,
            instance: "fixture".into(),
            input_hash: ContentHash::digest(id.to_le_bytes()),
            input: format!("input {id}"),
        });
    }
    Ok(Host {
        batch: Batch {
            lease: Lease {
                fingerprint: Fingerprint::digest(b"fixture"),
                activation: ActivationId::generate(),
                dimensions: 2,
            },
            jobs,
        },
        trace: Vec::new(),
        selection_error: None,
        progress: Ok(Some("published")),
        panic_on_process: false,
    })
}
fn failed<T>(result: Result<T>) -> Error {
    match result {
        Ok(_) => panic!("expected failure"),
        Err(error) => error,
    }
}
fn same_error(actual: &Error, expected: &Error) -> Result<()> {
    assert_eq!(
        crate::error::json_bytes(actual)?,
        crate::error::json_bytes(expected)?
    );
    assert_eq!(actual.diagnostic(), expected.diagnostic());
    Ok(())
}

#[test]
fn successful_publication_restores_the_selected_cursor_after_processing() -> Result<()> {
    let mut backfill = Backfill::default();
    backfill.scan.after = 17;
    let mut host = host(&[41, 43])?;
    assert_eq!(backfill.step(&mut host, 7)?, Some("published"));
    assert_eq!(backfill.scan.after, 43);
    assert_eq!(backfill.scan.steps, 1);
    assert_eq!(
        host.trace,
        [
            Event::Select {
                after: 17,
                limit: 7
            },
            Event::Process(vec![41, 43]),
        ]
    );
    Ok(())
}

#[test]
fn yield_and_processing_errors_leave_the_same_batch_ready_for_reselection() -> Result<()> {
    let mut backfill = Backfill::default();
    let mut host = host(&[41, 43])?;
    host.progress = Ok(None);
    assert!(backfill.step(&mut host, 2)?.is_none());
    assert_eq!(backfill.scan.after, 40);
    let mut error = Error::embedding_request_failed()
        .caused_by(std::io::Error::other("private-process-failure"));
    error.request_id = Some("request\nidentity".into());
    error.file_committed = true;
    host.progress = Err(error.clone());
    same_error(&failed(backfill.step(&mut host, 2)), &error)?;
    assert_eq!(backfill.scan.after, 40);
    host.progress = Ok(Some("published"));
    assert_eq!(backfill.step(&mut host, 2)?, Some("published"));
    assert_eq!(backfill.scan.after, 43);
    assert_eq!(
        host.trace,
        [
            Event::Select { after: 0, limit: 2 },
            Event::Process(vec![41, 43]),
            Event::Select {
                after: 40,
                limit: 2
            },
            Event::Process(vec![41, 43]),
            Event::Select {
                after: 40,
                limit: 2
            },
            Event::Process(vec![41, 43]),
        ]
    );
    Ok(())
}

#[test]
fn cursor_is_rewound_before_entering_host_processing() -> Result<()> {
    let mut backfill = Backfill::default();
    let mut host = host(&[41, 43])?;
    host.panic_on_process = true;
    let result = catch_unwind(AssertUnwindSafe(|| backfill.step(&mut host, 2)));
    assert!(result.is_err());
    assert_eq!(backfill.scan.after, 40);
    assert_eq!(host.trace.last(), Some(&Event::Process(vec![41, 43])));
    Ok(())
}

#[test]
fn empty_selection_keeps_end_of_scan_wrap_without_processing() -> Result<()> {
    let mut backfill = Backfill::default();
    backfill.scan.after = 43;
    let mut host = host(&[])?;
    assert!(backfill.step(&mut host, 32)?.is_none());
    assert_eq!(backfill.scan.after, 0);
    assert_eq!(
        host.trace,
        [Event::Select {
            after: 43,
            limit: 32
        }]
    );
    Ok(())
}

#[test]
fn selection_errors_preserve_the_selectors_cursor_effects_and_skip_processing() -> Result<()> {
    let mut backfill = Backfill::default();
    let mut host = host(&[41, 43])?;
    let expected = Error::active_space_missing();
    host.selection_error = Some(expected.clone());
    same_error(&failed(backfill.step(&mut host, 4)), &expected)?;
    assert_eq!(backfill.scan.after, 43);
    assert_eq!(host.trace, [Event::Select { after: 0, limit: 4 }]);
    Ok(())
}

struct Recording {
    space: Space,
    calls: Vec<Vec<String>>,
    failure: Option<Error>,
    fence: Option<Arc<Health>>,
}
impl Embedder for Recording {
    fn space(&self) -> &Space {
        &self.space
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        self.calls.push(inputs.to_vec());
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if let Some(health) = &self.fence {
            health.fence();
        }
        Ok(inputs.iter().map(|_| Ok(vec![1.0, 0.0])).collect())
    }
}
fn seeded(count: usize) -> Result<(tempfile::TempDir, Comb, Recording)> {
    let root = tempfile::tempdir()?;
    let mut comb = Comb::init(root.path())?;
    let body = (0..count)
        .map(|i| format!("## Section {i}\nEvidence number {i}\n"))
        .collect::<Vec<_>>()
        .join("\n");
    comb.put_markdown(
        &"concepts/corpus".parse()?,
        &format!("---\ntitle: Corpus\n---\n{body}"),
        None,
        &Write::new(&Actor::Human, &RequestId::generate(), None)?,
    )?;
    let engine = Recording {
        space: Space {
            backend: "fixture".into(),
            model: "recording".into(),
            revision: "fixed".into(),
            dimensions: 2,
            document_prefix: "d:".into(),
            query_prefix: "q:".into(),
            preprocessing: "fixture".into(),
            normalization: "l2".into(),
        },
        calls: Vec::new(),
        failure: None,
        fence: None,
    };
    comb.activate_embeddings(engine.space(), false)?;
    Ok((root, comb, engine))
}

#[test]
fn offline_adapter_uses_fixed_32_input_batches_and_stops_on_an_empty_pass() -> Result<()> {
    let (_root, mut comb, mut engine) = seeded(65)?;
    assert_eq!(
        comb.index()?.embedding_coverage(Filter::default())?.pending,
        65
    );
    let mut backfill = Backfill::default();
    while backfill.offline_step(&mut comb, &mut engine)? {}
    assert_eq!(
        engine.calls.iter().map(Vec::len).collect::<Vec<_>>(),
        [32, 32, 1]
    );
    let coverage = comb.index()?.embedding_coverage(Filter::default())?;
    assert_eq!(
        (coverage.total, coverage.embedded, coverage.pending),
        (65, 65, 0)
    );
    assert!(!backfill.offline_step(&mut comb, &mut engine)?);
    assert_eq!(engine.calls.len(), 3);
    Ok(())
}

#[test]
fn offline_inference_error_retries_identical_inputs_without_publishing() -> Result<()> {
    let (_root, mut comb, mut engine) = seeded(35)?;
    let expected =
        Error::embedding_request_failed().caused_by(std::io::Error::other("private-offline-cause"));
    engine.failure = Some(expected.clone());
    let mut backfill = Backfill::default();
    same_error(
        &failed(backfill.offline_step(&mut comb, &mut engine)),
        &expected,
    )?;
    let coverage = comb.index()?.embedding_coverage(Filter::default())?;
    assert_eq!((coverage.embedded, coverage.pending), (0, 35));
    assert!(backfill.offline_step(&mut comb, &mut engine)?);
    assert_eq!(engine.calls[0], engine.calls[1]);
    assert_eq!(engine.calls[1].len(), 32);
    Ok(())
}

#[test]
fn offline_publication_failure_does_not_advance_past_the_unpublished_batch() -> Result<()> {
    let (_root, mut comb, mut engine) = seeded(2)?;
    let batch = comb.index()?.embedding_jobs(&mut Scan::default(), 32)?;
    let Some(first) = batch.jobs.first() else {
        panic!("expected pending inputs");
    };
    engine.fence = Some(comb.health.clone());
    let mut backfill = Backfill::default();
    let error = failed(backfill.offline_step(&mut comb, &mut engine));
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert_eq!(backfill.scan.after, first.id - 1);
    assert_eq!(engine.calls.len(), 1);
    assert_eq!(
        comb.index().err().map(|error| error.code),
        Some(ErrorCode::RecoveryRequired)
    );
    Ok(())
}

#[test]
fn activation_replacement_resets_the_shared_cursor_before_selecting_new_work() -> Result<()> {
    let (_root, mut comb, mut engine) = seeded(35)?;
    let mut backfill = Backfill::default();
    assert!(backfill.offline_step(&mut comb, &mut engine)?);
    assert!(backfill.scan.after > 0);
    engine.space.model = "replacement".into();
    comb.activate_embeddings(engine.space(), true)?;
    assert!(backfill.offline_step(&mut comb, &mut engine)?);
    assert_eq!(engine.calls[0], engine.calls[1]);
    let coverage = comb.index()?.embedding_coverage(Filter::default())?;
    assert_eq!((coverage.embedded, coverage.pending), (32, 3));
    assert!(backfill.offline_step(&mut comb, &mut engine)?);
    assert_eq!(engine.calls[2].len(), 3);
    assert!(!backfill.offline_step(&mut comb, &mut engine)?);
    assert_eq!(backfill.scan.after, 0);
    Ok(())
}
