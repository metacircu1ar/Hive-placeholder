#![allow(clippy::unwrap_used)]
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
pub struct Server {
    pub url: String,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1/embeddings", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let task = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                };
                // Accepted sockets can inherit O_NONBLOCK on macOS. This
                // fixture uses blocking reads bounded by the timeout below.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    bytes.push(byte[0]);
                    assert!(bytes.len() < 8192);
                }
                // A cancelled/probe connection may close before sending a request.
                // Partial or malformed requests must still fail this fixture.
                if bytes.is_empty() {
                    continue;
                }
                assert!(bytes.ends_with(b"\r\n\r\n"));
                let header = String::from_utf8(bytes).unwrap();
                let len: usize = header
                    .lines()
                    .find_map(|s| {
                        s.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(str::trim)
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!(len < 65536);
                let mut body = vec![0; len];
                stream.read_exact(&mut body).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let count = request["input"].as_array().unwrap().len();
                let body = serde_json::to_vec(&serde_json::json!({"data":(0..count).map(|index| serde_json::json!({"index":index,"embedding":[1.0,0.0]})).collect::<Vec<_>>()})).unwrap();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            url,
            stop,
            task: Some(task),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
