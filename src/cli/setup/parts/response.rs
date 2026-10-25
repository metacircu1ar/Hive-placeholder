//! Child metadata is untrusted; only known forwarded coordinates enter public errors.
use super::{Result, invalid};
use crate::{cli::input::Context, error::Error};
use hive_input::InputError;
use hive_setup_support::runner::Output;
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rejection {
    error: InputError,
}

pub(super) fn decode<T: DeserializeOwned>(inputs: &[&str], output: Output) -> Result<T> {
    if output.code != 0 {
        let error =
            forwarded(inputs, &output).unwrap_or_else(|| invalid("Setup child command failed."));
        return Err(error.with_source(std::io::Error::other(format!(
            "Setup child exited {}: stdout: {}; stderr: {}",
            output.code, output.stdout, output.stderr
        ))));
    }
    let decoded = serde_json::from_str(&output.stdout).map_err(|cause| {
        invalid("Invalid structured response from setup child.").with_source(cause)
    })?;
    // Native progress is intentionally human stderr, never part of the JSON reply.
    if !output.stderr.is_empty() {
        eprint!("{}", output.stderr);
    }
    Ok(decoded)
}

fn forwarded(inputs: &[&str], output: &Output) -> Option<Error> {
    if output.code != 2 {
        return None;
    }
    let rejection: Rejection = crate::observability::optional(
        serde_json::from_str(&output.stdout),
        "setup_child_rejection",
    )?;
    let child = rejection.error.field();
    if !inputs.contains(&child) {
        return None;
    }
    let (flag, expected) = match child {
        "--root" => ("--comb-root", "a valid separate knowledge root"),
        "--embedding-config" => ("--embedding-config", "a valid HTTP embedding configuration"),
        "--bridge-file" => ("--bridge-file", "a separate Hive-to-Comb connection file"),
        "--service-environment" => (
            "--service-environment",
            "a valid private service environment file",
        ),
        "--service-path" => (
            "--service-path",
            "a colon-separated list of absolute existing directories",
        ),
        _ => return None,
    };
    Some(
        Context::new("setup", flag)
            .invalid(
                rejection.error.code(),
                "Invalid forwarded setup option.",
                expected,
            )
            .error,
    )
}

#[cfg(test)]
mod tests;
