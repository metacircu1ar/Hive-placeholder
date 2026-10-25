use super::{WRITE_TIMEOUT, test_support::RpcHarness};
use serde_json::json;
use std::{
    io::Read,
    os::{fd::AsRawFd, fd::OwnedFd, unix::net::UnixStream},
    process::ChildStdin,
    time::Duration,
};

#[test]
fn large_frame_reaches_an_active_reader_through_a_small_buffer() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::new(launcher);
    let (writer, mut reader) = UnixStream::pair().unwrap();
    let capacity: libc::c_int = 4096;
    assert_eq!(
        // SAFETY: writer is a live socket; capacity points to an initialized integer
        // whose exact size is passed to setsockopt, which retains neither pointer.
        unsafe {
            libc::setsockopt(
                writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                std::ptr::from_ref(&capacity).cast(),
                libc::socklen_t::try_from(std::mem::size_of_val(&capacity)).unwrap(),
            )
        },
        0
    );
    writer.set_nonblocking(true).unwrap();
    reader
        .set_read_timeout(Some(WRITE_TIMEOUT + Duration::from_secs(5)))
        .unwrap();
    let original = harness
        .rpc
        .pipes
        .child
        .stdin
        .replace(ChildStdin::from(OwnedFd::from(writer)));
    let reading = std::thread::spawn(move || {
        let mut received = Vec::new();
        reader.read_to_end(&mut received).unwrap();
        received
    });
    // The same worst-case escaping as the maximum joined reply: 24 MiB on wire.
    let frame = json!({"input": "\u{1}".repeat(4 * 1024 * 1024)});
    let mut expected = serde_json::to_vec(&frame).unwrap();
    expected.push(b'\n');
    let result = harness.rpc.write(frame);
    // Close the socket even on a failed write, then restore the capture child's
    // stdin so its lifetime remains under RpcHarness's normal cleanup.
    harness.rpc.pipes.child.stdin = original;
    let received = reading.join().unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(received.len(), expected.len());
    assert!(received == expected, "outgoing frame bytes changed");
}
