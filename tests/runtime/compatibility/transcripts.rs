use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
};

#[test]
fn synthetic_release_control_transcript_refuses_skew_before_dispatch() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../compat/synthetic-0/transcripts.json")).unwrap();
    let mut f = Harness::new();
    f.start();
    for case in fixture["control"].as_array().unwrap() {
        let mut stream = UnixStream::connect(runtime::socket_path(&f.root)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(stream, "{}", case["request"]).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        for (pointer, expected) in case["expect"].as_object().unwrap() {
            assert_eq!(response.pointer(pointer), Some(expected), "{response}");
        }
    }
    assert_eq!(f.ok(json!({"op":"agents"}))["agents"], json!([]));
    f.shutdown();
}
