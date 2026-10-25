//! Literal native image envelopes retain UUID-based acceptance and text-only shape.
use super::*;
use outgoing::{EncodedImage, OutgoingRequest, TurnInput};

fn image_turn(thread: &str, encoded: bool) -> OutgoingRequest {
    let OutgoingRequest::TurnStart(mut turn) = outgoing::turn_input(thread, "describe") else {
        unreachable!()
    };
    turn.input.push(TurnInput::LocalImage {
        path: "/owned/a.png".into(),
        claude: encoded.then(|| EncodedImage {
            kind: "base64",
            media_type: "image/png".into(),
            data: "cGl4ZWxz".into(),
        }),
    });
    OutgoingRequest::TurnStart(turn)
}
#[test]
fn native_image_wire_formats_and_replay_ack() {
    let mut f = Fixture::new();
    let request = image_turn(&f.id, true);
    let codex = serde_json::to_value(outgoing::request(3, request.clone())).unwrap();
    assert_eq!(
        codex["params"]["input"],
        json!([
            {"type":"text","text":"describe"}, {"type":"localImage","path":"/owned/a.png"}
        ])
    );
    let frames = f.codec.send(outgoing::request(3, request)).unwrap();
    let turn = frames[0]["uuid"].as_str().unwrap();
    assert_eq!(
        frames,
        vec![json!({"type":"user","uuid":turn,"session_id":f.id,
        "message":{"role":"user","content":[{"type":"text","text":"describe"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"cGl4ZWxz"}}]}})]
    );
    assert!(f.codec.pop().is_none());
    f.codec.receive(json!({"type":"system","subtype":"init","session_id":f.id,"model":"claude-sonnet-5","permissionMode":"default"})).unwrap();
    f.codec
        .receive(json!({"type":"user","uuid":turn,"session_id":f.id,"isReplay":true}))
        .unwrap();
    assert_eq!(f.codec.pop().unwrap()["id"], 3);
    assert_eq!(f.codec.pop().unwrap()["params"]["turn"]["id"], turn);
}
#[test]
fn unprepared_image_is_rejected_before_submission() {
    let mut f = Fixture::new();
    assert!(
        f.codec
            .send(outgoing::request(3, image_turn(&f.id, false)))
            .is_err()
    );
    let frames = f
        .codec
        .send(outgoing::request(4, outgoing::turn_input(&f.id, "plain")))
        .unwrap();
    assert_eq!(frames[0]["message"]["content"], "plain");
}

#[test]
fn exact_encoded_frame_limit_precedes_any_submission_state() {
    let mut f = Fixture::new();
    let mut plain = Fixture::new();
    let OutgoingRequest::TurnStart(mut turn) = image_turn(&f.id, true) else {
        unreachable!()
    };
    turn.input[0] = TurnInput::Text {
        text: "\"\n\t\\".repeat(13),
    };
    let frame = plain
        .codec
        .start_turn_bounded(3, turn.clone(), usize::MAX)
        .unwrap()
        .remove(0);
    let limit = serde_json::to_vec(&frame).unwrap().len();
    assert!(f.codec.before_input());
    assert!(
        f.codec
            .start_turn_bounded(3, turn.clone(), limit - 1)
            .is_err()
    );
    assert!(f.codec.before_input());
    let accepted = f.codec.start_turn_bounded(3, turn, limit).unwrap();
    assert_eq!(serde_json::to_vec(&accepted[0]).unwrap().len(), limit);
    assert!(!f.codec.before_input());
}
