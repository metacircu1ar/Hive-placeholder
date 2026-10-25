//! An input over its embedding model's limit is embedded in overlapping pieces.
//! Each piece repeats the input's title and heading; the chunk's vector is the
//! length-weighted mean of the pieces' vectors. Inputs that fit never reach
//! here, so their vectors are unchanged.
use super::{BATCH_SIZE, Embedder, normalize};
use crate::{Error, Result};

/// Halvings of a body that is still rejected: at most eight pieces.
const MAX_DEPTH: usize = 3;
/// A body piece shorter than this many bytes is not split again.
const MIN_PIECE_BYTES: usize = 64;
/// Bytes a piece of one unbroken run of text shares with its neighbor.
const RUN_OVERLAP_BYTES: usize = 16;

/// Embed `input` (`title\nheading\nbody`, without the document prefix) in
/// pieces of its body. The outer error fails the batch, as a transport error
/// does; the inner result is the row's: its vector, or the first rejection or
/// `rejected` when the body cannot be split small enough.
pub(super) fn embed(
    embedder: &mut (impl Embedder + ?Sized),
    input: &str,
    rejected: Error,
) -> Result<Result<Vec<f32>>> {
    let mut parts = input.splitn(3, '\n');
    let (Some(title), Some(heading), Some(body)) = (parts.next(), parts.next(), parts.next())
    else {
        return Ok(Err(rejected));
    };
    let prefix = embedder.space().document_prefix.clone();
    let dimensions = embedder.space().dimensions;
    let mut pending = vec![body.to_owned()];
    let mut finished: Vec<(Vec<f32>, usize)> = Vec::new();
    for _ in 0..MAX_DEPTH {
        let mut pieces = Vec::new();
        for piece in &pending {
            let Some((first, second)) = halves(piece) else {
                return Ok(Err(rejected));
            };
            pieces.extend([first, second]);
        }
        let inputs: Vec<String> = pieces
            .iter()
            .map(|piece| format!("{prefix}{title}\n{heading}\n{piece}"))
            .collect();
        let mut results = Vec::with_capacity(inputs.len());
        for batch in inputs.chunks(BATCH_SIZE) {
            let vectors = embedder.embed(batch)?;
            if vectors.len() != batch.len() {
                return Err(Error::embedding_batch_result_count());
            }
            results.extend(vectors);
        }
        pending.clear();
        for (piece, result) in pieces.into_iter().zip(results) {
            match result {
                Ok(vector) => finished.push((normalize(vector, dimensions)?, piece.len())),
                Err(error) if error.is_input_rejection() => pending.push(piece),
                Err(error) => return Ok(Err(error)),
            }
        }
        if pending.is_empty() {
            return mean(&finished, dimensions).map(Ok);
        }
    }
    Ok(Err(rejected))
}

/// The length-weighted mean of unit vectors, normalized.
fn mean(vectors: &[(Vec<f32>, usize)], dimensions: usize) -> Result<Vec<f32>> {
    let mut sum = vec![0_f64; dimensions];
    for (vector, weight) in vectors {
        let weight = crate::convert::usize_f64(*weight);
        for (total, value) in sum.iter_mut().zip(vector) {
            *total += f64::from(*value) * weight;
        }
    }
    normalize(
        sum.into_iter().map(crate::convert::f64_f32).collect(),
        dimensions,
    )
}

/// Two overlapping halves of `piece`, each shorter than it: by words with a
/// tenth of them shared, or for one long run of text by characters. `None`
/// when the piece is too short to split.
fn halves(piece: &str) -> Option<(String, String)> {
    if piece.len() < 2 * MIN_PIECE_BYTES {
        return None;
    }
    let words: Vec<&str> = piece.split_whitespace().collect();
    if words.len() >= 4 {
        let overlap = (words.len() / 10).max(1);
        let middle = words.len() / 2;
        let end = (middle + overlap.div_ceil(2)).min(words.len() - 1);
        let start = middle.saturating_sub(overlap / 2).max(1);
        return Some((words[..end].join(" "), words[start..].join(" ")));
    }
    let boundary = |at: usize, up: bool| {
        let mut at = at.min(piece.len());
        while !piece.is_char_boundary(at) {
            at = if up { at + 1 } else { at - 1 };
        }
        at
    };
    let middle = piece.len() / 2;
    let end = boundary(middle + RUN_OVERLAP_BYTES / 2, true);
    let start = boundary(middle - RUN_OVERLAP_BYTES / 2, false);
    Some((piece[..end].to_owned(), piece[start..].to_owned()))
}

#[cfg(test)]
mod tests;
