//! Full CLI branching with deterministic native conversation histories.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../../../tests/runtime/harness/mod.rs"]
mod harness;
use crate::integration_support as support;
use crate::{
    config::{Method, Profile, Provider},
    runtime,
    state::State,
};
use harness::Harness;
use serde_json::{Value, json};
#[cfg(target_os = "macos")]
use std::process::Stdio;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;
fn cli_binary() -> &'static std::path::Path {
    crate::auth::cli_tests::child::binary()
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
#[path = "../../../tests/runtime/branching.rs"]
mod branching;

mod claude {
    use super::*;
    pub(super) fn fixture() -> Harness {
        Harness::builder(Provider::Claude).build()
    }
    pub(super) fn spawn(f: &Harness, body: &str) -> String {
        let value = f.call(json!({"op":"spawn", "role":"claude", "cwd":f.tmp.path(), "body":body}));
        value["agent_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{value}"))
            .into()
    }
}

#[path = "../../../tests/runtime/api_keys/branching.rs"]
mod api_branching;
#[path = "../../../tests/runtime/api_keys/fixture.rs"]
mod api_fixture;

#[test]
fn malformed_claude_fork_never_executes_the_completion_tool_or_replays_input() {
    let mut f = claude::fixture();
    f.start();
    let source = claude::spawn(&f, "reply:prefix");
    f.wait(&source, |agent| {
        agent["last_finished_turn"]["outcome"] == "completed" && agent["state"] == "idle"
    });
    f.set_scenario(
        Provider::Claude,
        json!({"branch_disconnected_history":true}),
    );
    let accepted = f.call(json!({"op":"spawn","from_agent":source,"body":"review-ok"}));
    let child = accepted["agent_id"].as_str().unwrap();
    let refused = f.wait(child, |agent| {
        agent["branch_phase"] == "uncertain" && agent["state"] == "stopped"
    });
    assert_ne!(refused["state"], "completed");
    assert!(
        !f.events(child)
            .events
            .iter()
            .any(|event| event.kind == "completed")
    );
    let home = f.provider_home(Provider::Claude);
    let inputs = || {
        fs::read_to_string(home.join("inputs.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|row| row["thread"] == refused["thread_id"])
            .count()
    };
    assert_eq!(inputs(), 1);
    f.shutdown();
    f.start();
    let resume = f.call(json!({"op":"resume","target":child}));
    assert!(!resume["error"].is_null(), "{resume}");
    assert_eq!(inputs(), 1);
    f.shutdown();
}
