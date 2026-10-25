use super::*;
use serde_json::json;
use std::io;

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    fail_after: Option<usize>,
    fail_flush: bool,
    flushes: usize,
}
impl Write for Writer {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let available = self
            .fail_after
            .map_or(3, |n| n.saturating_sub(self.bytes.len()));
        if available == 0 {
            return Err(io::Error::other("injected write"));
        }
        let length = available.min(3).min(input.len());
        self.bytes.extend_from_slice(&input[..length]);
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        if self.fail_flush {
            Err(io::Error::other("injected flush"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn runtime_output_observes_short_partial_initial_and_flush_failures() {
    let expected = b"{\"ok\":true}\n";
    for (fail_after, fail_flush) in [
        (None, false),
        (Some(0), false),
        (Some(3), false),
        (None, true),
    ] {
        for suppressed in [false, true] {
            let mut stdout = Writer {
                fail_after,
                fail_flush,
                ..Writer::default()
            };
            let mut stderr = Writer {
                fail_after: Some(0),
                ..Writer::default()
            };
            let value = json!({"ok": true});
            let result = if suppressed {
                Exit::Suppressed(value)
            } else {
                Exit::Json(value)
            };
            let code = emit_to(Ok(result), &mut stdout, &mut stderr);
            assert_eq!(
                code,
                u8::from(!suppressed && (fail_after.is_some() || fail_flush))
            );
            assert_eq!(
                stdout.bytes,
                expected[..fail_after.unwrap_or(expected.len())]
            );
            assert_eq!(stdout.flushes, usize::from(fail_after.is_none()));
            assert_eq!(stderr.flushes, 0);
        }
    }
}

#[test]
fn failed_channels_are_independent_and_operational_errors_keep_their_bytes() {
    let error = || comb::Error::from(io::Error::other("PRIVATE cause"));
    let expected = render(Err(error().into()));
    assert!(expected.stderr.contains("PRIVATE cause"));
    assert!(!expected.stdout.contains("PRIVATE cause"));
    for fail_stdout in [false, true] {
        let mut stdout = Writer {
            fail_after: fail_stdout.then_some(3),
            ..Writer::default()
        };
        let mut stderr = Writer {
            fail_after: (!fail_stdout).then_some(0),
            ..Writer::default()
        };
        assert_eq!(emit_to(Err(error().into()), &mut stdout, &mut stderr), 1);
        if fail_stdout {
            assert_eq!(stderr.bytes, expected.stderr.as_bytes());
            assert_eq!(stdout.bytes, expected.stdout.as_bytes()[..3]);
        } else {
            assert_eq!(stdout.bytes, expected.stdout.as_bytes());
            assert!(stderr.bytes.is_empty());
        }
    }
}

#[test]
fn text_silent_and_mcp_paths_use_only_their_channels() {
    let mut stdout = Writer::default();
    let mut stderr = Writer::default();
    assert_eq!(
        emit_to(
            Ok(Exit::Text("Comb example\n".into())),
            &mut stdout,
            &mut stderr
        ),
        0
    );
    assert_eq!(stdout.bytes, b"Comb example\n");
    assert_eq!(stdout.flushes, 1);
    assert_eq!(stderr.flushes, 0);
    let mut broken = Writer {
        fail_after: Some(0),
        ..Writer::default()
    };
    assert_eq!(emit_to(Ok(Exit::Silent), &mut broken, &mut stderr), 0);
    assert_eq!(emit_mcp(Ok(Exit::Silent), &mut broken), 0);
    for (fail_after, fail_flush) in [
        (None, false),
        (Some(0), false),
        (Some(3), false),
        (None, true),
    ] {
        let mut stderr = Writer {
            fail_after,
            fail_flush,
            ..Writer::default()
        };
        let error = comb::Error::from(io::Error::other("PRIVATE cause"));
        assert_eq!(emit_mcp(Err(error.into()), &mut stderr), 1);
        let expected = super::super::input::Failure::from(comb::Error::from(io::Error::other(
            "PRIVATE cause",
        )))
        .render_mcp()
        .stderr;
        assert!(!expected.contains("PRIVATE"));
        assert!(expected.contains("io_error:"));
        assert!(expected.contains("comb mcp --credential"));
        assert_eq!(
            stderr.bytes,
            expected.as_bytes()[..fail_after.unwrap_or(expected.len())]
        );
        assert_eq!(stderr.flushes, usize::from(fail_after.is_none()));
    }
}

#[test]
fn mcp_typed_errors_keep_input_status_only_when_stderr_completes() {
    for (fail_after, fail_flush) in [
        (None, false),
        (Some(0), false),
        (Some(3), false),
        (None, true),
    ] {
        let mut stderr = Writer {
            fail_after,
            fail_flush,
            ..Writer::default()
        };
        let error = super::super::input::Context::new("mcp", "--credential")
            .credential(std::path::Path::new("/PRIVATE-missing/credential.json"))
            .err()
            .unwrap();
        let code = emit_mcp(Err(error), &mut stderr);
        assert_eq!(
            code,
            if fail_after.is_some() || fail_flush {
                1
            } else {
                2
            }
        );
        assert!(!String::from_utf8_lossy(&stderr.bytes).contains("PRIVATE"));
        if fail_after.is_none() {
            assert!(String::from_utf8_lossy(&stderr.bytes).contains("--credential"));
        }
    }
}
