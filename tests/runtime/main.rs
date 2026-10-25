#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // Integration fixture helpers panic on failure.

mod api_keys;
mod harness;
#[path = "../support/mod.rs"]
mod support;

use harness::Harness;
use hive::{
    config::{Method, Profile, Provider},
    runtime,
    state::State,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn cli_binary() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_BIN_EXE_hivectl"))
}

fn configure_runtime(f: &Harness, settings: Value, other: bool) {
    f.edit_config(|config| {
        config["runtime"] = settings;
        if other {
            config["auth_profiles"]["other"] = config["auth_profiles"]["main"].clone();
        }
    });
    if other {
        State::new(f.root.clone())
            .unwrap()
            .register(
                "other",
                &Profile {
                    provider: Provider::Codex,
                    method: Method::Subscription,
                    api_key_env: None,
                    api_key_file: None,
                },
            )
            .unwrap();
    }
}

macro_rules! provider_matrix {
    ($name:ident, $case:path) => {
        mod $name {
            use super::*;
            #[test]
            fn codex() {
                $case(Provider::Codex);
            }
            #[test]
            fn claude() {
                $case(Provider::Claude);
            }
        }
    };
}

mod approvals;
mod attach;
mod attachments;
mod auth_balance;
mod auth_fallback;
mod calls;
#[cfg(feature = "comb")]
mod comb;
mod compaction;
mod compatibility;
mod context_inputs;
mod control;
mod daemon_start;
mod external_mcp;
mod follow;
mod gc;
mod joins;
mod lifecycle;
mod lifetimes;
mod live;
mod logging;
mod messaging;
mod models;
mod names;
mod panes;
mod provider;
mod purge;
mod recovery;
mod redirect;
mod scheduled;
mod scheduling;
mod selection_inputs;
mod stalls;
mod steering;
mod tui;
mod usage;

mod claude;

mod responsiveness;
