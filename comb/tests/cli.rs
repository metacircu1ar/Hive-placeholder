#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
use serde_json::Value;

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[path = "cli/config_input.rs"]
mod config_input;
#[path = "cli/credential_output.rs"]
mod credential_output;
#[path = "cli/diagnostics.rs"]
mod diagnostics;
#[path = "cli/embed_replacement.rs"]
mod embed_replacement;
#[path = "support/embedding_http.rs"]
mod embedding_http;
#[path = "cli/embedding_startup.rs"]
mod embedding_startup;
#[path = "cli/http_options.rs"]
mod http_options;
#[path = "cli/logging.rs"]
mod logging;
#[path = "cli/model_selectors.rs"]
mod model_selectors;
#[path = "cli/mutations.rs"]
mod mutations;
#[path = "cli/paths.rs"]
mod paths;
#[path = "cli/payload.rs"]
mod payload;
#[path = "cli/runtime_guidance.rs"]
mod runtime_guidance;
#[path = "cli/support.rs"]
mod support;
#[path = "cli/values.rs"]
mod values;
#[path = "cli/wire_input.rs"]
mod wire_input;
use support::cli;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn version_identifies_compiled_embedding_backend_without_opening_state() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .current_dir(directory.path())
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("comb {} (http)\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
