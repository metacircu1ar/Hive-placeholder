use super::*;

struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

fn respond(listener: TcpListener, header: &'static str, stall: bool, hold: mpsc::Receiver<()>) {
    let mut socket = accept(&listener);
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let n: usize = headers
        .lines()
        .find_map(|l| {
            l.to_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_owned)
        })
        .unwrap()
        .parse()
        .unwrap();
    socket.read_exact(&mut vec![0; n]).unwrap();
    socket.write_all(header.as_bytes()).unwrap();
    if stall {
        hold.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
}

// Keep the real 1 s HTTP timeout, <4 s completion bound, 3 s socket-read
// watchdog and 5 s server-release watchdog. Each policy has its own case.
fn rejected_response(header: &'static str, stall: bool, code: crate::ErrorCode) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/embeddings", listener.local_addr().unwrap());
    let (release, hold) = mpsc::channel();
    let mut release = Release(Some(release));
    let server = thread::spawn(move || respond(listener, header, stall, hold));
    let mut engine = Engine::load(Config::Http(HttpConfig {
        model: "m".into(),
        revision: "r".into(),
        url,
        dimensions: 2,
        document_prefix: String::new(),
        query_prefix: String::new(),
        api_key_env: None,
        timeout_seconds: 1,
        request_dimensions: false,
        tokenizer: None,
        max_tokens: None,
    }))
    .unwrap();
    let started = std::time::Instant::now();
    let result = engine.embed(&["one".into()]);
    if stall {
        release.0.take().unwrap().send(()).unwrap();
    }
    server.join().unwrap();
    assert_eq!(result.err().unwrap().code, code);
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn oversized_http_response_is_rejected() {
    rejected_response(
        "HTTP/1.1 200 OK\r\nContent-Length: 9000000\r\n\r\n",
        false,
        crate::ErrorCode::InvalidEmbedding,
    );
}

#[test]
fn http_redirect_is_not_followed() {
    rejected_response(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\n\r\n",
        false,
        crate::ErrorCode::EmbeddingTransport,
    );
}

#[test]
fn stalled_http_body_obeys_the_total_request_deadline() {
    rejected_response(
        "HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n",
        true,
        crate::ErrorCode::EmbeddingTransport,
    );
}

fn accept(listener: &TcpListener) -> std::net::TcpStream {
    listener.set_nonblocking(true).unwrap();
    let started = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((socket, _)) => return socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < std::time::Duration::from_secs(5),
                    "HTTP fixture accept watchdog expired"
                );
                thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(error) => panic!("HTTP fixture accept failed: {error}"),
        }
    }
}
