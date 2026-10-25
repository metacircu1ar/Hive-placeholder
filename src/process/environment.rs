//! Provider environment isolation and executable selection.
use super::{BTreeMap, Command, Error, OsStr, OsString, Path, PathBuf, Provider, Result};
use crate::os;
use serde_json::Value;
use std::{env, fs};

pub(crate) struct EnvironmentVariable {
    pub(crate) key: OsString,
    pub(crate) value: OsString,
}
pub(crate) fn child_environment(
    provider: Provider,
    home: &Path,
    inherited: impl Iterator<Item = EnvironmentVariable>,
) -> BTreeMap<OsString, OsString> {
    let mut environment: BTreeMap<OsString, OsString> = inherited
        .filter(|EnvironmentVariable { key, .. }| {
            key.to_str().is_some_and(|key| {
                matches!(
                    key,
                    "PATH"
                        | "USER"
                        | "LOGNAME"
                        | "SHELL"
                        | "LANG"
                        | "TERM"
                        | "COLORTERM"
                        | "TMPDIR"
                        | "DISPLAY"
                        | "WAYLAND_DISPLAY"
                        | "XDG_RUNTIME_DIR"
                        | "DBUS_SESSION_BUS_ADDRESS"
                        | "BROWSER"
                        | "HTTP_PROXY"
                        | "HTTPS_PROXY"
                        | "ALL_PROXY"
                        | "NO_PROXY"
                        | "http_proxy"
                        | "https_proxy"
                        | "all_proxy"
                        | "no_proxy"
                        | "SSL_CERT_FILE"
                        | "SSL_CERT_DIR"
                        | "NODE_EXTRA_CA_CERTS"
                ) || key.starts_with("LC_")
            })
        })
        .map(|EnvironmentVariable { key, value }| (key, value))
        .collect();
    let process_home = if cfg!(target_os = "macos") && provider == Provider::Claude {
        env::var_os("HOME").unwrap_or_else(|| home.as_os_str().to_owned())
    } else {
        home.as_os_str().to_owned()
    };
    environment.insert("HOME".into(), process_home);
    environment.insert(
        match provider {
            Provider::Codex => "CODEX_HOME",
            Provider::Claude => "CLAUDE_CONFIG_DIR",
        }
        .into(),
        home.as_os_str().to_owned(),
    );
    if provider == Provider::Claude {
        environment.insert(
            "ANTHROPIC_CONFIG_DIR".into(),
            home.join("anthropic").into_os_string(),
        );
    }
    environment
}

pub(crate) fn executable(command: &OsStr) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = Path::new(command);
    let candidates: Vec<PathBuf> = if path.is_absolute() {
        vec![path.into()]
    } else if path.components().count() != 1 {
        return Err(Error::invalid_executable(
            "Provider executable must be an absolute path or a command name.",
        ));
    } else {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .filter(|p| p.is_absolute())
            .map(|p| p.join(command))
            .collect()
    };
    candidates
        .into_iter()
        .find(|p| {
            p.metadata()
                .inspect_err(|error| {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        crate::observability::observation_failed("executable_metadata", error);
                    }
                })
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & os::EXECUTABLE_BITS != 0)
        })
        .ok_or(Error::missing_binary(
            "Provider executable is not installed or is not executable.",
        ))
}

pub(crate) fn command(binary: &Path, provider: Provider, home: &Path, work: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .envs(child_environment(
            provider,
            home,
            env::vars_os().map(|(key, value)| EnvironmentVariable { key, value }),
        ))
        .current_dir(work);
    match provider {
        Provider::Codex => {
            command.args(["-c", "cli_auth_credentials_store=\"file\""]);
        }
        Provider::Claude => {
            command.args(["--setting-sources", ""]);
        }
    }
    command
}

/// Bypass only the recognized npm entrypoint selected by the default command.
/// Explicit paths and arbitrary wrappers retain their exact launch semantics.
pub(crate) fn runtime_codex_executable(command: &OsStr) -> Result<PathBuf> {
    let fallback = executable(command)?;
    if command != OsStr::new("codex") {
        return Ok(fallback);
    }
    let Some(entry) =
        crate::observability::optional(fallback.canonicalize(), "provider_wrapper_path")
    else {
        return Ok(fallback);
    };
    if entry.file_name() != Some(OsStr::new("codex.js")) {
        return Ok(fallback);
    }
    let Some(package) = entry.parent().and_then(Path::parent) else {
        return Ok(fallback);
    };
    let manifest = crate::observability::optional(
        fs::read(package.join("package.json")),
        "provider_wrapper_manifest",
    )
    .and_then(|v| {
        crate::observability::optional(
            serde_json::from_slice::<Value>(&v),
            "provider_wrapper_parse",
        )
    });
    if manifest.as_ref().and_then(|v| v["name"].as_str()) != Some("@openai/codex") {
        return Ok(fallback);
    }
    let target = match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ("codex-darwin-arm64", "aarch64-apple-darwin"),
        ("macos", "x86_64") => ("codex-darwin-x64", "x86_64-apple-darwin"),
        ("linux", "aarch64") => ("codex-linux-arm64", "aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => ("codex-linux-x64", "x86_64-unknown-linux-musl"),
        _ => return Ok(fallback),
    };
    for vendor in [
        package
            .join("node_modules/@openai")
            .join(target.0)
            .join("vendor"),
        package.join("vendor"),
    ] {
        let path = vendor.join(target.1).join("bin/codex");
        if let Some(binary) =
            crate::observability::optional(executable(path.as_os_str()), "native_wrapper_candidate")
        {
            return Ok(binary);
        }
    }
    Ok(fallback)
}
