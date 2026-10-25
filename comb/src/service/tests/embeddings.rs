use super::*;

use crate::test_support::embedding::FakeEmbedder;

fn fake(delay: Duration) -> FakeEmbedder {
    let space = serde_json::from_value(json!({"backend":"fixture","model":"deterministic","revision":"fixed","dimensions":2,"document_prefix":"d:","query_prefix":"q:","preprocessing":"test","normalization":"l2-f32"})).unwrap();
    FakeEmbedder::new(space, |_| Ok(vec![1.0, 0.0]))
        .hold_on("holdquery")
        .document_delay(delay)
}

fn query(text: &str, mode: Mode) -> Operation {
    Operation::Search {
        request: Search {
            query: text.into(),
            ..Default::default()
        },
        mode,
    }
}

pub(super) async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

mod cache;
mod coalescing;
mod deadlines;
mod http;
mod reactor;
mod scheduling;

fn coverage(state: &State) -> embedding::Coverage {
    state
        .snapshot()
        .unwrap()
        .index
        .embedding_coverage(crate::search::Filter::default())
        .unwrap()
}

fn needle_corpus(count: usize) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for i in 0..count {
        write!(text, "## Heading {i}\nNeedle observation {i}\n\n").unwrap();
    }
    text
}

fn simultaneous_searches(human: &Connection) -> Vec<tokio::task::JoinHandle<Result<Value>>> {
    let barrier = Arc::new(tokio::sync::Barrier::new(64));
    let mut searches = Vec::new();
    for n in 0..64 {
        let gate = barrier.clone();
        let client = human.clone();
        searches.push(tokio::spawn(async move {
            gate.wait().await;
            call(&client, &query(&format!("needle {n}"), Mode::Hybrid), None).await
        }));
    }
    searches
}

struct ModelRelease(Arc<(Mutex<bool>, std::sync::Condvar)>);
impl Drop for ModelRelease {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = false;
        self.0.1.notify_all();
    }
}
