use super::*;
use crate::runtime::{
    domain::MessageState,
    provider::protocol::outgoing::{self, OutgoingRequest, TurnInput},
    store::Message,
};
use std::{fs, io::Cursor};

fn png() -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}
fn captured(bytes: &[u8], name: &str) -> (tempfile::TempDir, Attachment) {
    let temp = crate::runtime::test_directory();
    let private = temp.path().join("state");
    crate::state::directory(&private).unwrap();
    let source = temp.path().join(name);
    fs::write(&source, bytes).unwrap();
    let mut capture = Capture::new(private.join("attachments"), private).unwrap();
    let item = capture.message(&[source]).unwrap().remove(0);
    capture.publish().unwrap();
    (temp, item)
}
fn message(items: Vec<Attachment>) -> Message {
    Message {
        joined: false,
        has_attachments: !items.is_empty(),
        attachments: items,
        id: "message".into(),
        sender: "human".into(),
        recipient: "agent".into(),
        body: "describe \"this\"\nplease".into(),
        state: MessageState::Submitting,
    }
}
fn prepared(items: Vec<Attachment>, codex: bool, limit: usize) -> Result<delivery::Prepared> {
    let OutgoingRequest::TurnStart(turn) = outgoing::turn_start("thread", "model", "low", None, "")
    else {
        unreachable!()
    };
    delivery::prepare(message(items), turn, codex, limit)
}
#[test]
fn native_preparation_decodes_real_images_and_keeps_exact_owned_references() {
    let bytes = png();
    let (_temp, mut item) = captured(&bytes, "image.log");
    images::select(std::slice::from_mut(&mut item), images::Usage::default());
    for codex in [true, false] {
        let out = prepared(
            vec![item.clone()],
            codex,
            crate::limits::JOIN_TRANSPORT_BYTES,
        )
        .unwrap();
        let OutgoingRequest::TurnStart(turn) = out.request else {
            panic!()
        };
        assert!(
            matches!(&turn.input[0],TurnInput::Text{text} if text.contains("image.log") && text.contains("native_image"))
        );
        let TurnInput::LocalImage { path, claude } = &turn.input[1] else {
            panic!()
        };
        assert_eq!(fs::read(path).unwrap(), bytes);
        if codex {
            assert!(claude.is_none());
        } else {
            use base64::Engine;
            let data = claude.as_ref().unwrap();
            assert_eq!(data.media_type, "image/png");
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(&data.data)
                    .unwrap(),
                bytes
            );
        }
    }
}
#[test]
fn malformed_images_fall_back_and_snapshot_corruption_is_an_error() {
    for (name, bytes) in [
        (
            "truncated.png",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".as_slice(),
        ),
        (
            "truncated.jpg",
            b"\xff\xd8\xff\xc0\0\x11\x08\0\x01\0\x01".as_slice(),
        ),
    ] {
        let (_temp, mut item) = captured(bytes, name);
        images::select(std::slice::from_mut(&mut item), images::Usage::default());
        assert_eq!(item.delivery, Delivery::NativeImage);
        let out = prepared(vec![item], false, crate::limits::JOIN_TRANSPORT_BYTES).unwrap();
        assert_eq!(
            out.message.attachments[0].delivery,
            Delivery::Reference {
                reason: ReferenceReason::InvalidImage
            }
        );
        let OutgoingRequest::TurnStart(turn) = out.request else {
            panic!()
        };
        assert_eq!(turn.input.len(), 1);
    }
    let (_temp, mut item) = captured(&png(), "real.png");
    images::select(std::slice::from_mut(&mut item), images::Usage::default());
    let Source::Snapshot { path, .. } = &item.source else {
        panic!()
    };
    fs::write(path, "changed").unwrap();
    assert_eq!(
        prepared(vec![item], true, crate::limits::JOIN_TRANSPORT_BYTES)
            .err()
            .unwrap()
            .code(),
        "state_inconsistent"
    );
}
#[test]
fn image_limits_include_exact_dimensions_bytes_occurrences_and_retained_budget() {
    let (_temp, mut item) = captured(&png(), "real.png");
    item.dimensions = Some([2000, 2000]);
    item.bytes = 3 * 1024 * 1024;
    assert_eq!(item.native_bytes().unwrap(), images::IMAGE_BYTES);
    item.dimensions = Some([2001, 1]);
    assert_eq!(item.native_bytes(), Err(ReferenceReason::ImageDimensions));
    item.dimensions = Some([1, 1]);
    item.bytes += 1;
    assert_eq!(item.native_bytes(), Err(ReferenceReason::ImageBytes));
    item.bytes -= 1;
    let mut items = vec![item.clone(); 3];
    assert_eq!(
        images::select(&mut items, images::Usage::default()),
        images::Usage {
            bytes: images::TURN_BYTES,
            images: 2
        }
    );
    assert_eq!(
        items[2].delivery,
        Delivery::Reference {
            reason: ReferenceReason::MessageImageBudget
        }
    );
    item.bytes = 1;
    let mut items = vec![item; 2];
    assert_eq!(
        images::select(
            &mut items,
            images::Usage {
                bytes: 0,
                images: 19
            }
        )
        .images,
        1
    );
    assert_eq!(
        items[1].delivery,
        Delivery::Reference {
            reason: ReferenceReason::ConversationImageBudget
        }
    );
    assert_eq!(
        images::select(
            &mut items,
            images::Usage {
                bytes: images::HISTORY_BYTES,
                images: 0
            }
        )
        .images,
        0
    );
}
#[test]
fn actual_escaped_frame_budget_falls_back_before_submission() {
    let (_temp, mut item) = captured(&png(), "quotes-\"\n.png");
    images::select(std::slice::from_mut(&mut item), images::Usage::default());
    let out = prepared(
        vec![item.clone()],
        false,
        crate::limits::JOIN_TRANSPORT_BYTES,
    )
    .unwrap();
    let OutgoingRequest::TurnStart(turn) = out.request else {
        panic!()
    };
    let frame = crate::runtime::provider::claude::user_frame(
        "thread",
        "00000000-0000-0000-0000-000000000000",
        turn.input,
    )
    .unwrap();
    let size = frame.to_string().len();
    assert_eq!(
        prepared(vec![item.clone()], false, size)
            .unwrap()
            .message
            .attachments[0]
            .delivery,
        Delivery::NativeImage
    );
    let fallback = prepared(vec![item], false, size - 1).unwrap();
    assert_eq!(
        fallback.message.attachments[0].delivery,
        Delivery::Reference {
            reason: ReferenceReason::FrameBudget
        }
    );
    let OutgoingRequest::TurnStart(turn) = fallback.request else {
        panic!()
    };
    assert_eq!(turn.input.len(), 1);
}
