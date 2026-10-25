mod admission;
mod import;

mod golden;

mod poison;

mod removal;

mod values;

use super::*;

use crate::{
    Actor,
    embedding::Space,
    search::{Mode, Search},
    store::Write,
};

use auth::{Claim, Connection, Grant};

use serde_json::{Value, json};

use tokio::io::AsyncWriteExt;

use tokio::net::UnixStream;

mod embeddings;

mod mcp;

mod lifecycle;
mod publication;
mod socket;
mod support;
use support::{Running, call, claim, get, issue, put};
pub(super) use support::{runtime, setup};
mod compatibility;
mod startup;
