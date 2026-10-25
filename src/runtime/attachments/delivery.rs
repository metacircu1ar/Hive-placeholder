//! File reads, bounded image decoding and exact native-frame sizing run off the writer.
use super::{Attachment, Delivery, ReferenceReason as Reason, Source, images};
use crate::{
    error::{Error, Result},
    runtime::{
        provider::{
            claude,
            protocol::outgoing::{self, EncodedImage, OutgoingRequest, TurnInput, TurnStart},
        },
        store::Message,
    },
    state,
};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};

pub(in crate::runtime) struct Prepared {
    pub message: Message,
    pub request: OutgoingRequest,
}

pub(in crate::runtime) fn prepare(
    mut message: Message,
    mut turn: TurnStart,
    codex: bool,
    frame_limit: usize,
) -> Result<Prepared> {
    let images = message
        .attachments
        .iter_mut()
        .filter(|item| item.delivery == Delivery::NativeImage)
        .map(|item| image(item, codex))
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>>>()?;
    turn.input = vec![TurnInput::Text {
        text: message.input(),
    }];
    turn.input.extend(images);
    let mut request = OutgoingRequest::TurnStart(turn);
    if frame_bytes(&request, codex)? > frame_limit {
        for item in &mut message.attachments {
            if item.delivery == Delivery::NativeImage {
                item.delivery = Delivery::Reference {
                    reason: Reason::FrameBudget,
                };
            }
        }
        let OutgoingRequest::TurnStart(turn) = &mut request else {
            unreachable!()
        };
        turn.input = vec![TurnInput::Text {
            text: message.input(),
        }];
        if frame_bytes(&request, codex)? > frame_limit {
            return Err(Error::output_limit(
                "Attachment references exceed the provider frame budget.",
            ));
        }
    }
    Ok(Prepared { message, request })
}
fn image(item: &mut Attachment, codex: bool) -> Result<Option<TurnInput>> {
    let Source::Snapshot { path, sha256, .. } = &item.source else {
        return Err(Error::state_inconsistent(
            "Native image is not an owned snapshot.",
        ));
    };
    if item.native_bytes().is_err() {
        return Err(Error::state_inconsistent(
            "Native image exceeds its stored eligibility limits.",
        ));
    }
    let mut bytes = Vec::new();
    state::private_reader(path)?
        .take(item.bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::state_io_from)?;
    if bytes.len() as u64 != item.bytes || format!("{:x}", Sha256::digest(&bytes)) != *sha256 {
        return Err(Error::state_inconsistent(
            "Stored attachment content no longer matches its digest.",
        ));
    }
    if !valid_image(&bytes, item.dimensions) {
        item.delivery = Delivery::Reference {
            reason: Reason::InvalidImage,
        };
        return Ok(None);
    }
    let claude = (!codex).then(|| EncodedImage {
        kind: "base64",
        media_type: item.media_type.clone(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    });
    Ok(Some(TurnInput::LocalImage {
        path: path.clone(),
        claude,
    }))
}
fn valid_image(bytes: &[u8], expected: Option<[u32; 2]>) -> bool {
    let Some(mut reader) = crate::observability::optional(
        image::ImageReader::new(Cursor::new(bytes)).with_guessed_format(),
        "attachment_image_format",
    ) else {
        return false;
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(images::MAX_DIMENSION);
    limits.max_image_height = Some(images::MAX_DIMENSION);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    crate::observability::optional(reader.decode(), "attachment_image_decode")
        .is_some_and(|decoded| Some([decoded.width(), decoded.height()]) == expected)
}
fn frame_bytes(request: &OutgoingRequest, codex: bool) -> Result<usize> {
    let frame = if codex {
        crate::runtime::api::value(outgoing::request(u64::MAX, request.clone()))
    } else {
        let OutgoingRequest::TurnStart(turn) = request else {
            unreachable!()
        };
        // Native Claude generates UUID strings of the same exact length.
        claude::user_frame(
            &turn.thread_id,
            "00000000-0000-0000-0000-000000000000",
            turn.input.clone(),
        )?
    };
    Ok(frame.to_string().len())
}
