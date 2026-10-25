//! Ordinary deterministic engines; model-specific and ownership fixtures stay local.
use crate::{
    Error, Result,
    embedding::{Embedder, Space},
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub(crate) struct FakeEmbedder {
    space: Space,
    row: fn(&str) -> Result<Vec<f32>>,
    failure: Option<Error>,
    document_delay: Option<Duration>,
    held_text: Option<&'static str>,
    gate: Arc<(Mutex<bool>, Condvar)>,
    calls: Arc<AtomicUsize>,
    query_calls: Arc<AtomicUsize>,
    inputs: usize,
}

impl FakeEmbedder {
    pub(crate) fn new(space: Space, row: fn(&str) -> Result<Vec<f32>>) -> Self {
        Self {
            space,
            row,
            failure: None,
            document_delay: None,
            held_text: None,
            gate: Arc::new((Mutex::new(true), Condvar::new())),
            calls: Arc::new(AtomicUsize::new(0)),
            query_calls: Arc::new(AtomicUsize::new(0)),
            inputs: 0,
        }
    }

    pub(crate) fn document_delay(mut self, delay: Duration) -> Self {
        self.document_delay = Some(delay);
        self
    }

    pub(crate) fn hold_on(mut self, text: &'static str) -> Self {
        self.held_text = Some(text);
        self
    }

    pub(crate) fn batch_error(mut self, error: Error) -> Self {
        self.failure = Some(error);
        self
    }

    pub(crate) fn calls(&self) -> Arc<AtomicUsize> {
        self.calls.clone()
    }

    pub(crate) fn query_calls(&self) -> Arc<AtomicUsize> {
        self.query_calls.clone()
    }

    pub(crate) fn input_count(&self) -> usize {
        self.inputs
    }

    pub(crate) fn gate(&self) -> Arc<(Mutex<bool>, Condvar)> {
        self.gate.clone()
    }

    fn record_call(&mut self, inputs: &[String]) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs += inputs.len();
    }

    fn wait_for_release(&self, inputs: &[String]) {
        if self
            .held_text
            .is_some_and(|text| inputs.iter().any(|input| input.contains(text)))
        {
            let (lock, ready) = &*self.gate;
            let mut blocked = lock.lock().unwrap();
            while *blocked {
                blocked = ready.wait(blocked).unwrap();
            }
        }
    }
}

impl Embedder for FakeEmbedder {
    fn space(&self) -> &Space {
        &self.space
    }

    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        if let Some(error) = self.failure.clone() {
            self.record_call(inputs);
            return Err(error);
        }
        if inputs
            .first()
            .is_some_and(|input| input.starts_with(&self.space.query_prefix))
        {
            self.query_calls.fetch_add(1, Ordering::SeqCst);
        }
        self.record_call(inputs);
        self.wait_for_release(inputs);
        if inputs
            .first()
            .is_some_and(|input| input.starts_with(&self.space.document_prefix))
            && let Some(delay) = self.document_delay
        {
            std::thread::sleep(delay);
        }
        Ok(inputs.iter().map(|input| (self.row)(input)).collect())
    }
}

mod tests;
