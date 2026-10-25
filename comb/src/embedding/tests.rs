mod http_limits;
mod publication;

use super::*;
use crate::test_support::{TestComb, embedding::FakeEmbedder};
use crate::values::{CreateSafety, FindingKind, Route};
use crate::{
    Actor, Comb,
    search::{Mode, QueryVector, Search},
    store::Write,
};
use std::{
    io::{Read, Write as IoWrite},
    net::TcpListener,
    sync::mpsc,
    thread,
};

fn mock_embedder(model: &str) -> FakeEmbedder {
    FakeEmbedder::new(
        Space {
            backend: "deterministic-test-only".into(),
            model: model.into(),
            revision: "fixture".into(),
            dimensions: 2,
            document_prefix: "document: ".into(),
            query_prefix: "query: ".into(),
            preprocessing: "fixture".into(),
            normalization: "l2".into(),
        },
        mock_row,
    )
}

fn mock_row(input: &str) -> Result<Vec<f32>> {
    if input.contains("OVERSIZED") {
        return Err(Error::new(
            crate::ErrorCode::EmbeddingInputTooLarge,
            "fixture",
        ));
    }
    Ok(if input.contains("feline") || input.contains("kitten") {
        vec![1.0, 0.0]
    } else {
        vec![0.0, 1.0]
    })
}

fn setup() -> (tempfile::TempDir, Comb, FakeEmbedder) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let mock = mock_embedder("a");
    comb.activate_embeddings(mock.space(), false).unwrap();
    (root, comb, mock)
}
fn put(comb: &mut Comb, slug: &str, title: &str, text: &str) {
    let markdown = format!("---\ntitle: {title}\ntype: component\n---\n{text}\n");
    let hash = match comb.get(&slug.parse().unwrap()) {
        Ok(document) => Some(document.hash),
        Err(error) if error.is_not_found() => None,
        Err(error) => panic!("fixture page read failed: {error}"),
    };
    comb.put_markdown(
        &slug.parse().unwrap(),
        &markdown,
        hash.as_ref(),
        &Write::new(
            &Actor::Agent {
                bridge: "mock".into(),
                id: "writer".into(),
                role: None,
                execution: "exec".into(),
                turn: "turn".into(),
            },
            &crate::values::RequestId::generate(),
            None,
        )
        .unwrap(),
    )
    .unwrap();
}
fn fill(comb: &mut Comb, mock: &mut impl Embedder) {
    let mut scan = Scan::default();
    loop {
        let batch = comb
            .index()
            .unwrap()
            .embedding_jobs(&mut scan, BATCH_SIZE)
            .unwrap();
        if batch.jobs.is_empty() {
            break;
        }
        let completed = run_batch(mock, batch).unwrap();
        comb.apply_embeddings(completed).unwrap();
    }
}
fn request(query: &str) -> Search {
    Search {
        query: query.into(),
        ..Search::default()
    }
}
fn query(comb: &Comb, mock: &mut impl Embedder, text: &str) -> Query {
    let lease = comb.index().unwrap().embedding_lease(mock.space()).unwrap();
    run_query(mock, lease, text).unwrap()
}

fn coverage(comb: &Comb) -> Coverage {
    comb.index()
        .unwrap()
        .embedding_coverage(request("x").filter())
        .unwrap()
}

fn complete_pending(comb: &Comb, engine: &mut impl Embedder) -> Completed {
    let batch = comb
        .index()
        .unwrap()
        .embedding_jobs(&mut Scan::default(), 32)
        .unwrap();
    run_batch(engine, batch).unwrap()
}

struct Blocked {
    inner: FakeEmbedder,
    started: mpsc::Sender<()>,
    resume: mpsc::Receiver<()>,
}
impl Embedder for Blocked {
    fn space(&self) -> &Space {
        self.inner.space()
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        self.started.send(()).unwrap();
        self.resume.recv().unwrap();
        self.inner.embed(inputs)
    }
}

fn many_chunks(count: usize, marker: &str) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for n in 0..count {
        write!(text, "## Entry {n}\nfeline {marker} observation {n}\n\n").unwrap();
    }
    text
}

fn measure(count: usize) -> u64 {
    let (_root, mut comb, mut mock) = setup();
    put(
        &mut comb,
        "concepts/corpus",
        "Corpus",
        &many_chunks(count, "initial"),
    );
    let mut scan = Scan::default();
    loop {
        let batch = comb.index().unwrap().embedding_jobs(&mut scan, 32).unwrap();
        if batch.jobs.is_empty() {
            break;
        }
        comb.apply_embeddings(run_batch(&mut mock, batch).unwrap())
            .unwrap();
    }
    assert_eq!(
        comb.index()
            .unwrap()
            .embedding_coverage(request("x").filter())
            .unwrap()
            .embedded,
        count as u64
    );
    scan.steps
}

fn http_reply(
    body: String,
    extra: &str,
    configure: impl FnOnce(&mut Config),
) -> (Engine, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/embeddings", listener.local_addr().unwrap());
    let extra = extra.to_owned();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
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
        let length: usize = headers
            .lines()
            .find_map(|l| {
                l.to_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .unwrap()
            .parse()
            .unwrap();
        let mut body_in = vec![0; length];
        socket.read_exact(&mut body_in).unwrap();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
        .unwrap();
        String::from_utf8(body_in).unwrap()
    });
    let mut config = Config::Http(HttpConfig {
        model: "fixture".into(),
        revision: "immutable-fixture".into(),
        url,
        dimensions: 2,
        document_prefix: String::new(),
        query_prefix: String::new(),
        api_key_env: None,
        timeout_seconds: 2,
        request_dimensions: false,
        tokenizer: None,
        max_tokens: None,
    });
    configure(&mut config);
    (Engine::load(config).unwrap(), handle)
}

mod cases_behaviour_1;
mod cases_indexing_1;
mod cases_learning_1;
