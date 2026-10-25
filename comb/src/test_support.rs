//! Test-only adapters for exercising the same ready checks and index paths.
use crate::{Comb, Result};

impl Comb {
    pub(crate) fn graph(&self, slug: &str, depth: usize) -> Result<Vec<String>> {
        self.ready()?;
        self.index
            .graph(&slug.parse()?, depth)
            .map(|slugs| slugs.into_iter().map(String::from).collect())
    }
}

mod comb;
pub(crate) mod embedding;
#[path = "../tests/support/embedding_http.rs"]
pub(crate) mod embedding_http;
mod page;
mod unix;
pub(crate) use comb::{TestComb, private_directory};
pub(crate) use page::PageBuilder;
pub(crate) use unix::fifo;

pub(crate) fn agent(
    bridge: &str,
    id: &str,
    role: Option<&str>,
    execution: &str,
    turn: &str,
) -> crate::Actor {
    crate::Actor::Agent {
        bridge: bridge.into(),
        id: id.into(),
        role: role.map(str::to_owned),
        execution: execution.into(),
        turn: turn.into(),
    }
}

pub(crate) fn client(id: &str, name: &str) -> crate::Actor {
    crate::Actor::Client {
        id: id.into(),
        name: name.into(),
    }
}

pub(crate) fn write<'a>(
    actor: &'a crate::Actor,
    id: &str,
    run: Option<&'a str>,
) -> Result<crate::store::Write<'a>> {
    crate::store::Write::new(actor, &id.parse()?, run)
}

macro_rules! assert_code {
    ($result:expr, $code:expr $(, $message:expr)* $(,)?) => {{
        let Err(error) = ($result) else {
            panic!("expected an error from {}", stringify!($result));
        };
        assert_eq!(error.code, $code $(, $message)*);
        error
    }};
}
pub(crate) use assert_code;

pub(crate) fn assert_input_cause(error: &crate::Error, field: &str, expected: crate::Error) {
    use std::error::Error as _;
    let input = error.input().expect("typed authored input rejection");
    assert_eq!(input.code(), hive_input::Code::InvalidValue);
    assert_eq!(input.field(), field);
    let cause = error
        .source()
        .unwrap()
        .downcast_ref::<crate::Error>()
        .unwrap();
    assert_eq!(cause.code, expected.code);
    assert_eq!(cause.message, expected.message);
    assert!(!error.file_committed);
}

#[cfg(test)]
mod tests;

pub(crate) fn optional_bytes(path: impl AsRef<std::path::Path>) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("fixture snapshot read failed: {error}"),
    }
}
