use super::*;
use crate::service::auth::ConnectionExt as _;
use crate::test_support::TestComb;

pub(in crate::service) fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

pub(in crate::service) fn setup(
    engine: Option<Box<dyn Embedder>>,
) -> (tempfile::TempDir, Server, Connection) {
    let (dir, comb) = TestComb::short().unwrap().into_parts();
    let socket = dir.path().canonicalize().unwrap().join("comb.sock");
    let human = Connection::human(dir.path(), socket.clone()).unwrap();
    let server = Server::from_comb(
        comb,
        Options {
            socket: Some(socket),
            vector_cache_mib: engine.is_some().then_some(1),
            ..Default::default()
        },
        engine,
    )
    .unwrap()
    .bind()
    .unwrap();
    (dir, server, human)
}

pub(super) async fn call(
    connection: &Connection,
    op: &Operation,
    claim: Option<Claim>,
) -> Result<Value> {
    client::call(connection, op, claim, Duration::from_secs(10)).await
}

pub(super) fn put(slug: &str, text: &str, id: &str) -> Operation {
    Operation::Put {
        slug: slug.parse().unwrap(),
        markdown: format!("---\ntitle: Shared writer\ntype: component\n---\n{text}\n"),
        expected_hash: None,
        request_id: id.parse().unwrap(),
        run: None,
    }
}

pub(super) fn claim(id: &str, role: &str) -> Claim {
    Claim::Agent {
        id: id.into(),
        role: Some(role.into()),
        execution: "execution".into(),
        turn: "turn".into(),
    }
}

pub(super) async fn issue(human: &Connection, grant: Grant) -> Connection {
    Connection {
        format: hive_input::compatibility::Exact,
        socket: human.socket.clone(),
        credential: serde_json::from_value(
            call(human, &Operation::CredentialCreate { grant }, None)
                .await
                .unwrap(),
        )
        .unwrap(),
    }
}

pub(super) struct Running {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl Running {
    pub(super) fn start(server: Server) -> Self {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        Self {
            stop: Some(stop),
            task: Some(task),
        }
    }

    pub(super) async fn finish(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.task.take().unwrap().await.unwrap().unwrap();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub(super) async fn get(connection: &Connection, slug: &str) -> Value {
    call(
        connection,
        &Operation::Get {
            slug: slug.parse().unwrap(),
        },
        None,
    )
    .await
    .unwrap()
}
