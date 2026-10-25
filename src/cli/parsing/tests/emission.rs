use super::*;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    fail_after: Option<usize>,
    fail_flush: bool,
    flushes: usize,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.fail_after.unwrap_or(usize::MAX) - self.bytes.len();
        if remaining == 0 {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let length = bytes.len().min(remaining).min(3);
        self.bytes.extend_from_slice(&bytes[..length]);
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        if self.fail_flush {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}
fn outcome() -> Exit {
    Exit {
        stdout: "{\"ok\":true}\n".into(),
        stderr: "safe human text\n".into(),
        code: 0,
    }
}

#[test]
fn checked_channels_handle_short_writes_and_flush_each_nonempty_stream() {
    let exit = outcome();
    let mut stdout = Output::default();
    let mut stderr = Output::default();
    assert_eq!(emit(&exit, &mut stdout, &mut stderr), 0);
    assert_eq!(stdout.bytes, exit.stdout.as_bytes());
    assert_eq!(stderr.bytes, exit.stderr.as_bytes());
    assert_eq!((stdout.flushes, stderr.flushes), (1, 1));
}

#[test]
fn write_or_flush_failure_never_prevents_the_other_channel_or_retries_output() {
    for fail_flush in [false, true] {
        for output_fails in [false, true] {
            let exit = outcome();
            let mut failed = Output {
                fail_after: (!fail_flush).then_some(3),
                fail_flush,
                ..Output::default()
            };
            let mut healthy = Output::default();
            let code = if output_fails {
                emit(&exit, &mut failed, &mut healthy)
            } else {
                emit(&exit, &mut healthy, &mut failed)
            };
            assert_eq!(code, 1);
            assert_eq!(
                healthy.bytes,
                if output_fails {
                    exit.stderr.as_bytes()
                } else {
                    exit.stdout.as_bytes()
                }
            );
            let expected = if output_fails {
                &exit.stdout
            } else {
                &exit.stderr
            };
            assert_eq!(
                failed.bytes,
                &expected.as_bytes()[..if fail_flush { expected.len() } else { 3 }]
            );
            assert_eq!(failed.flushes, usize::from(fail_flush));
        }
    }
}

#[test]
fn recorded_service_suppression_survives_broken_diagnostic_channels() {
    let mut exit = outcome();
    exit.code = 1;
    let mut failed = Output {
        fail_after: Some(0),
        ..Output::default()
    };
    let mut healthy = Output::default();
    assert_eq!(
        emit_with_suppression(&exit, &mut failed, &mut healthy, true),
        0
    );
    assert_eq!(healthy.bytes, exit.stderr.as_bytes());
    assert_eq!(
        emit_with_suppression(&exit, &mut failed, &mut Closed, true),
        0
    );
}

#[test]
fn empty_channels_are_not_written_or_flushed() {
    let mut exit = outcome();
    exit.stderr.clear();
    let mut unused = Output {
        fail_after: Some(0),
        fail_flush: true,
        ..Output::default()
    };
    assert_eq!(emit(&exit, &mut Vec::new(), &mut unused), 0);
    assert_eq!(unused.flushes, 0);
    assert!(unused.bytes.is_empty());
}

#[test]
fn preparation_metadata_failure_is_operational_not_input() {
    let failure = super::super::super::input::Context::new("unknown-command", "--body").invalid(
        Code::InvalidValue,
        "Invalid body.",
        "nonempty text",
    );
    let exit = file_failure(&failure);
    assert_eq!(exit.code, 1);
    assert!(exit.stdout.contains("internal_error"));
    assert!(!exit.stdout.contains("field"));
}

#[test]
fn operational_human_output_escapes_controls_is_bounded_and_omits_causes() {
    let error = super::super::super::Error::state_io("Unsafe\npath\r\x1b[31m\t\u{202e}end")
        .with_source(io::Error::other("PRIVATE cause"));
    let line = human(&error);
    assert_eq!(
        line,
        "Hive: state_io: Unsafe\\npath\\r\\u{1b}[31m\\t\\u{202e}end\n"
    );
    assert!(!line.contains("PRIVATE"));
    let exit = file_failure(&error.clone().into());
    assert_eq!(exit.stderr, line);
    let value: serde_json::Value = serde_json::from_str(&exit.stdout).unwrap();
    assert_eq!(value["error"]["message"], error.message());
    let error = super::super::super::Error::StateIo {
        message: "x".repeat(1000).into(),
        source: None,
    };
    let line = human(&error);
    assert!(line.ends_with("...\n"));
    assert_eq!(line.len(), "Hive: state_io: ".len() + 204);
}
