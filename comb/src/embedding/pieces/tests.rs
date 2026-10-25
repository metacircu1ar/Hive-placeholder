//! Oversized inputs: splitting, weighting and bounded rejection.
use super::*;
use crate::embedding::Space;

/// Embeds inputs of at most `limit` bytes as the counts of 'a' and 'b'.
struct Limited {
    space: Space,
    limit: usize,
    calls: usize,
    inputs: usize,
}
impl Limited {
    fn new(limit: usize) -> Self {
        Self {
            space: Space {
                backend: "deterministic-test-only".into(),
                model: "limited".into(),
                revision: "fixture".into(),
                dimensions: 2,
                document_prefix: "document: ".into(),
                query_prefix: "query: ".into(),
                preprocessing: "fixture".into(),
                normalization: "l2".into(),
            },
            limit,
            calls: 0,
            inputs: 0,
        }
    }
}
impl Embedder for Limited {
    fn space(&self) -> &Space {
        &self.space
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        self.calls += 1;
        self.inputs += inputs.len();
        Ok(inputs
            .iter()
            .map(|input| {
                if input.len() > self.limit {
                    return Err(Error::http_input_too_large(self.limit));
                }
                let count = |c| crate::convert::usize_f64(input.matches(c).count());
                Ok(vec![
                    crate::convert::f64_f32(count('a') + 1.0),
                    crate::convert::f64_f32(count('b') + 1.0),
                ])
            })
            .collect())
    }
}

fn words(count: usize, word: &str) -> String {
    vec![word; count].join(" ")
}

#[test]
fn halves_cover_the_text_overlap_and_shrink() {
    let text = (0..100)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (first, second) = halves(&text).unwrap();
    assert!(first.len() < text.len() && second.len() < text.len());
    assert!(text.starts_with(&first) && text.ends_with(&second));
    // The halves share words around the middle.
    assert!(first.contains("w50") && second.contains("w49"));
    assert_eq!(halves(&text), Some((first, second)));
    // One long run without spaces, including multi-byte characters.
    let run = "認識".repeat(100);
    let (first, second) = halves(&run).unwrap();
    assert!(first.len() < run.len() && second.len() < run.len());
    assert!(run.starts_with(&first) && run.ends_with(&second));
    assert_eq!(halves("short text"), None);
}

#[test]
fn an_oversized_input_is_embedded_as_the_weighted_mean_of_its_pieces() {
    let mut embedder = Limited::new(400);
    let body = format!("{} {}", words(60, "aaa"), words(60, "bbb"));
    let input = format!("Title\nHeading\n{body}");
    let vector = embed(&mut embedder, &input, Error::http_input_too_large(400))
        .unwrap()
        .unwrap();
    // Both halves fit; their mean leans on neither side.
    assert!((vector[0] - vector[1]).abs() < 0.05, "{vector:?}");
    let norm: f32 = vector.iter().map(|v| v * v).sum();
    assert!((norm - 1.0).abs() < 1e-5);
    assert_eq!(embedder.inputs, 2);
}

#[test]
fn rows_that_cannot_fit_keep_their_rejection_within_a_bounded_number_of_calls() {
    // The title and heading alone exceed the limit: every piece is rejected.
    let mut embedder = Limited::new(40);
    let input = format!("{}\nHeading\n{}", "T".repeat(64), words(200, "aaa"));
    let result = embed(&mut embedder, &input, Error::http_input_too_large(40)).unwrap();
    assert_eq!(
        result.unwrap_err().code,
        crate::ErrorCode::EmbeddingInputTooLarge
    );
    assert!(embedder.inputs <= 2 + 4 + 8, "{}", embedder.inputs);
    // Without a body line there is nothing to split.
    let mut embedder = Limited::new(10);
    let result = embed(
        &mut embedder,
        "only a title",
        Error::http_input_too_large(10),
    )
    .unwrap();
    assert!(result.is_err());
    assert_eq!(embedder.calls, 0);
}

#[test]
fn run_batch_embeds_oversized_rows_and_keeps_fitting_rows_unchanged() {
    let mut embedder = Limited::new(400);
    let small = "Title\nHeading\naaa bbb".to_owned();
    let large = format!("Title\nHeading\n{}", words(150, "aaa"));
    let fitting = embedder
        .embed(&[format!("document: {small}")])
        .unwrap()
        .pop()
        .unwrap()
        .unwrap();
    let mut rows = [Ok(fitting.clone()), Err(Error::http_input_too_large(400))];
    for (input, row) in [&small, &large].into_iter().zip(rows.iter_mut()) {
        *row = match std::mem::replace(row, Ok(Vec::new())) {
            Err(rejected) if rejected.is_input_rejection() => {
                embed(&mut embedder, input, rejected).unwrap()
            }
            other => other,
        };
    }
    assert_eq!(rows[0].as_ref().unwrap(), &fitting);
    assert!(rows[1].is_ok());
}
