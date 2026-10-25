use super::{Classified, Transport};
use crate::{Error, ErrorCode, Result};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

struct Step {
    inputs: Vec<String>,
    reply: std::result::Result<Vec<Vec<f32>>, Classified>,
}
impl Step {
    fn success(inputs: &[&str], values: &[f32]) -> Self {
        Self {
            inputs: inputs.iter().map(|text| (*text).into()).collect(),
            reply: Ok(values.iter().map(|value| vec![*value]).collect()),
        }
    }
    fn failure(inputs: &[&str], error: Classified) -> Self {
        Self {
            inputs: inputs.iter().map(|text| (*text).into()).collect(),
            reply: Err(error),
        }
    }
    fn status(inputs: &[&str], status: u16) -> Self {
        let Err(error) = Classified::status(status) else {
            panic!("expected rejected status");
        };
        Self::failure(inputs, error)
    }
}
struct Script {
    steps: VecDeque<Step>,
    deadline: Instant,
}
impl Script {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            deadline: Instant::now() + Duration::from_secs(5),
        }
    }
    fn embed(&mut self, texts: &[&str], prefix: &str) -> Result<Vec<Result<Vec<f32>>>> {
        let deadline = self.deadline;
        let result = super::embed(self, texts, prefix, deadline);
        assert!(self.steps.is_empty(), "expected requests were skipped");
        result
    }
}
impl Transport for Script {
    fn request(
        &mut self,
        texts: &[&str],
        deadline: Instant,
    ) -> std::result::Result<Vec<Vec<f32>>, Classified> {
        assert_eq!(
            deadline, self.deadline,
            "every branch shares the original deadline"
        );
        let Some(step) = self.steps.pop_front() else {
            panic!("unexpected request: {texts:?}");
        };
        assert_eq!(
            texts
                .iter()
                .map(|text| (*text).to_owned())
                .collect::<Vec<_>>(),
            step.inputs
        );
        step.reply
    }
}
fn vector(result: &Result<Vec<f32>>) -> &[f32] {
    match result {
        Ok(vector) => vector,
        Err(error) => panic!("unexpected row failure: {error}"),
    }
}
fn failed(result: Result<Vec<Result<Vec<f32>>>>) -> Error {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected whole-batch failure"),
    }
}
fn marked(error: Error, marker: &'static str) -> Error {
    error.caused_by(std::io::Error::other(marker))
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
fn successful_batch_is_returned_in_order_without_a_probe() -> Result<()> {
    let mut script = Script::new([Step::success(&["a", "b"], &[1.0, 2.0])]);
    let rows = script.embed(&["a", "b"], "doc:")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(vector(&rows[0]), [1.0]);
    assert_eq!(vector(&rows[1]), [2.0]);
    Ok(())
}

#[test]
fn odd_split_finishes_left_before_right_and_probes_only_once() -> Result<()> {
    let mut script = Script::new([
        Step::status(&["a", "b", "bad", "d", "e"], 422),
        Step::success(&["doc:test"], &[0.0]),
        Step::success(&["a", "b"], &[1.0, 2.0]),
        Step::status(&["bad", "d", "e"], 400),
        Step::status(&["bad"], 413),
        Step::success(&["d", "e"], &[4.0, 5.0]),
    ]);
    let rows = script.embed(&["a", "b", "bad", "d", "e"], "doc:")?;
    assert_eq!(rows.len(), 5);
    assert_eq!(vector(&rows[0]), [1.0]);
    assert_eq!(vector(&rows[1]), [2.0]);
    let Err(error) = &rows[2] else {
        panic!("expected isolated rejection");
    };
    same_error(error, &Error::embedding_input_rejected(413))?;
    assert_eq!(vector(&rows[3]), [4.0]);
    assert_eq!(vector(&rows[4]), [5.0]);
    Ok(())
}

#[test]
fn even_split_reuses_the_probe_across_both_recursive_halves() -> Result<()> {
    let mut script = Script::new([
        Step::status(&["a", "b", "c", "d"], 400),
        Step::success(&["doc:test"], &[0.0]),
        Step::status(&["a", "b"], 413),
        Step::success(&["a"], &[1.0]),
        Step::status(&["b"], 422),
        Step::status(&["c", "d"], 400),
        Step::success(&["c"], &[3.0]),
        Step::success(&["d"], &[4.0]),
    ]);
    let rows = script.embed(&["a", "b", "c", "d"], "doc:")?;
    assert_eq!(rows.len(), 4);
    assert_eq!(vector(&rows[0]), [1.0]);
    assert!(rows[1].is_err());
    assert_eq!(vector(&rows[2]), [3.0]);
    assert_eq!(vector(&rows[3]), [4.0]);
    Ok(())
}

#[test]
fn query_rejection_uses_document_probe_and_keeps_original_singleton_error() -> Result<()> {
    let mut rejection = marked(Error::embedding_input_rejected(422), "original-row-cause");
    rejection.request_id = Some("identity".into());
    rejection.file_committed = true;
    let mut script = Script::new([
        Step::failure(
            &["query:needle"],
            Classified::InputRejected(rejection.clone()),
        ),
        Step::success(&["document:test"], &[0.0]),
    ]);
    let rows = script.embed(&["query:needle"], "document:")?;
    assert_eq!(rows.len(), 1);
    let Err(error) = &rows[0] else {
        panic!("expected original row rejection");
    };
    same_error(error, &rejection)
}

#[test]
fn rejected_probe_is_an_endpoint_failure_with_the_probe_cause() -> Result<()> {
    for status in [400, 413, 422] {
        let probe = marked(
            Error::embedding_input_rejected(status),
            "probe-private-cause",
        );
        let expected = Error::embedding_endpoint_rejected().caused_by(probe.clone());
        let mut script = Script::new([
            Step::failure(
                &["a", "b"],
                Classified::InputRejected(marked(
                    Error::embedding_input_rejected(400),
                    "batch-only-cause",
                )),
            ),
            Step::failure(&["doc:test"], Classified::InputRejected(probe)),
        ]);
        let error = failed(script.embed(&["a", "b"], "doc:"));
        same_error(&error, &expected)?;
        assert_eq!(error.code, ErrorCode::EmbeddingEndpointRejected);
        assert!(!error.is_input_rejection());
        assert!(
            error
                .diagnostic()
                .is_some_and(|message| !message.contains("batch-only-cause"))
        );
    }
    Ok(())
}

#[test]
fn transport_failure_during_probe_keeps_its_full_error_without_splitting() -> Result<()> {
    let mut expected = marked(Error::embedding_request_failed(), "probe-transport-cause");
    expected.request_id = Some("request\nidentity".into());
    expected.file_committed = true;
    let mut script = Script::new([
        Step::status(&["a", "b"], 400),
        Step::failure(&["doc:test"], Classified::Transient(expected.clone())),
    ]);
    same_error(&failed(script.embed(&["a", "b"], "doc:")), &expected)
}

#[test]
fn auth_redirect_rate_limit_and_outage_statuses_abort_without_probe_or_bisection() -> Result<()> {
    for status in [100, 199, 300, 302, 401, 403, 429, 500, 503, 599] {
        let mut script = Script::new([Step::status(&["a", "b"], status)]);
        let error = failed(script.embed(&["a", "b"], "doc:"));
        same_error(&error, &Error::embedding_http_status(status))?;
        assert!(!error.is_input_rejection());
    }
    Ok(())
}

#[test]
fn malformed_deadline_size_and_send_failures_keep_their_full_errors() -> Result<()> {
    for error in [
        Error::malformed_embedding_response(),
        Error::embedding_batch_deadline(),
        Error::embedding_response_size(),
        Error::embedding_request_failed(),
    ] {
        let expected = marked(error, "private-whole-batch-cause");
        let mut script = Script::new([Step::failure(&["a", "b"], expected.clone().into())]);
        same_error(&failed(script.embed(&["a", "b"], "doc:")), &expected)?;
    }
    Ok(())
}

#[test]
fn right_branch_failure_discards_already_successful_left_results() -> Result<()> {
    let expected = marked(Error::malformed_embedding_response(), "right-branch-cause");
    let mut script = Script::new([
        Step::status(&["a", "b", "c"], 400),
        Step::success(&["doc:test"], &[0.0]),
        Step::success(&["a"], &[1.0]),
        Step::failure(&["b", "c"], expected.clone().into()),
    ]);
    same_error(&failed(script.embed(&["a", "b", "c"], "doc:")), &expected)
}

#[test]
fn budget_exhaustion_after_probe_and_left_work_does_not_start_a_new_deadline() -> Result<()> {
    let mut script = Script::new([
        Step::status(&["a", "b"], 413),
        Step::success(&["doc:test"], &[0.0]),
        Step::success(&["a"], &[1.0]),
        Step::failure(&["b"], Error::embedding_batch_deadline().into()),
    ]);
    same_error(
        &failed(script.embed(&["a", "b"], "doc:")),
        &Error::embedding_batch_deadline(),
    )
}
