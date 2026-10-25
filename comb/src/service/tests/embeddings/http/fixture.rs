use super::*;
use std::{
    io::{BufRead, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Condvar, atomic::AtomicBool},
};

#[derive(Default)]
struct Gate {
    held: Mutex<bool>,
    ready: Condvar,
    entered: AtomicUsize,
    waiting: AtomicBool,
}

impl Gate {
    fn hold(self: &Arc<Self>) -> Held {
        *self.held.lock().unwrap() = true;
        Held(self.clone())
    }

    fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.ready.notify_all();
    }

    fn wait(&self) {
        let held = self.held.lock().unwrap();
        if !*held {
            return;
        }
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.waiting.store(true, Ordering::SeqCst);
        // Failure cleanup must not leave an HTTP worker parked indefinitely.
        let (held, elapsed) = self
            .ready
            .wait_timeout_while(held, Duration::from_secs(5), |held| *held)
            .unwrap();
        self.waiting.store(false, Ordering::SeqCst);
        let still_held = *held;
        drop(held);
        assert!(
            !elapsed.timed_out() || !still_held,
            "HTTP fixture gate watchdog expired"
        );
    }
}

pub(super) struct Held(Arc<Gate>);
impl Drop for Held {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct Controls {
    status: Arc<AtomicUsize>,
    documents: Arc<Gate>,
    queries: Arc<Gate>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    stop: AtomicBool,
}

pub(super) struct HttpFixture {
    pub(super) config: embedding::Config,
    pub(super) status: Arc<AtomicUsize>,
    pub(super) active: Arc<AtomicUsize>,
    pub(super) max_active: Arc<AtomicUsize>,
    controls: Arc<Controls>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HttpFixture {
    pub(super) fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/embeddings", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let controls = Arc::new(Controls {
            status: Arc::new(AtomicUsize::new(200)),
            documents: Arc::new(Gate::default()),
            queries: Arc::new(Gate::default()),
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
            stop: AtomicBool::new(false),
        });
        let worker = controls.clone();
        let thread = std::thread::spawn(move || serve(listener, &worker));
        Self {
            config: config(url),
            status: controls.status.clone(),
            active: controls.active.clone(),
            max_active: controls.max_active.clone(),
            controls,
            thread: Some(thread),
        }
    }

    pub(super) fn hold_documents(&self) -> Held {
        self.controls.documents.hold()
    }
    pub(super) fn hold_queries(&self) -> Held {
        self.controls.queries.hold()
    }
    pub(super) fn documents_started(&self) -> usize {
        self.controls.documents.entered.load(Ordering::SeqCst)
    }
    pub(super) fn queries_started(&self) -> usize {
        self.controls.queries.entered.load(Ordering::SeqCst)
    }
    pub(super) fn document_waiting(&self) -> bool {
        self.controls.documents.waiting.load(Ordering::SeqCst)
    }
    pub(super) fn query_waiting(&self) -> bool {
        self.controls.queries.waiting.load(Ordering::SeqCst)
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.controls.documents.release();
        self.controls.queries.release();
        self.controls.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn config(url: String) -> embedding::Config {
    embedding::Config::Http(embedding::HttpConfig {
        model: "fixture".into(),
        revision: "fixed".into(),
        url,
        dimensions: 2,
        document_prefix: "d:".into(),
        query_prefix: "q:".into(),
        api_key_env: None,
        timeout_seconds: 5,
        request_dimensions: false,
        tokenizer: None,
        max_tokens: None,
    })
}

fn serve(listener: TcpListener, controls: &Arc<Controls>) {
    let mut requests = Vec::new();
    while !controls.stop.load(Ordering::SeqCst) {
        let (socket, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) => panic!("fixture accept failed: {error}"),
        };
        let controls = controls.clone();
        requests.push(std::thread::spawn(move || respond(socket, &controls)));
    }
    for request in requests {
        request.join().unwrap();
    }
}

fn request(socket: TcpStream) -> (std::io::BufReader<TcpStream>, Value) {
    // BSD accepts inherit O_NONBLOCK; this fixture uses blocking reads.
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut stream = std::io::BufReader::new(socket);
    let mut size = 0;
    loop {
        let mut line = String::new();
        if stream.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(length) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            size = length.trim().parse().unwrap();
        }
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).unwrap();
    let body = serde_json::from_slice(&bytes).unwrap();
    (stream, body)
}

fn respond(socket: TcpStream, controls: &Controls) {
    let (mut stream, body) = request(socket);
    let inputs = body["input"].as_array().unwrap();
    controls.max_active.fetch_max(
        controls.active.fetch_add(1, Ordering::SeqCst) + 1,
        Ordering::SeqCst,
    );
    if inputs
        .first()
        .is_some_and(|s| s.as_str().unwrap().starts_with("q:"))
    {
        controls.queries.wait();
    }
    if inputs
        .first()
        .is_some_and(|s| s.as_str().unwrap().starts_with("d:"))
    {
        controls.documents.wait();
    }
    let mut code = controls.status.load(Ordering::SeqCst);
    if code == 200
        && inputs
            .iter()
            .any(|s| s.as_str().unwrap().contains("REJECTME"))
    {
        code = 400;
    }
    let result = if code == 200 { json!({"data": inputs.iter().enumerate().map(|(i,_)|json!({"index":i,"embedding":[1.0,0.0]})).collect::<Vec<_>>()}) } else { json!({"error":"fixture rejection"}) }.to_string();
    let _ = write!(
        stream.get_mut(),
        "HTTP/1.1 {code} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{result}",
        result.len()
    );
    controls.active.fetch_sub(1, Ordering::SeqCst);
}
