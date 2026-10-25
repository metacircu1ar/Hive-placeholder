use super::*;
use crate::cli::response_tests::Stub;
use serde_json::json;
use std::io;

struct FailedOutput {
    bytes: Vec<u8>,
    failed: bool,
    flush_only: bool,
    flushes: usize,
}
impl Write for FailedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(!self.failed, "no writes after a failed page");
        if !self.flush_only && self.bytes.len() == 3 {
            self.failed = true;
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let length = if self.flush_only {
            bytes.len()
        } else {
            bytes.len().min(3 - self.bytes.len())
        };
        self.bytes.extend_from_slice(&bytes[..length]);
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        assert!(!self.failed, "no flush after a failed page");
        self.flushes += 1;
        self.failed = true;
        Err(io::Error::from_raw_os_error(libc::EIO))
    }
}

#[test]
fn failed_follow_write_or_flush_stops_before_cursor_advance_or_another_read() {
    for flush_only in [false, true] {
        let page = json!({
            "events":[{"seq":1,"agent_id":"a","execution_id":null,"timestamp":1,
                "type":"turn_finished","data":{}}],
            "cursor":1,"pruned_through":0,"history_expired":false
        });
        let expected = format!("{page}\n");
        let stub = Stub::new(vec![page]);
        let args = Read {
            target: "a".into(),
            after: None,
            limit: None,
            follow: true,
        };
        let mut output = FailedOutput {
            bytes: Vec::new(),
            failed: false,
            flush_only,
            flushes: 0,
        };
        assert!(matches!(follow(stub.root(), &args, &mut output),
            Ok(Some(Failure::Output(error))) if error.io_raw_os_error() == Some(libc::EIO)));
        assert_eq!(output.flushes, usize::from(flush_only));
        assert!(output.failed);
        assert_eq!(
            output.bytes,
            if flush_only {
                expected.as_bytes()
            } else {
                &expected.as_bytes()[..3]
            }
        );
        let requests = stub.finish();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["args"]["after"], 0);
    }
}
