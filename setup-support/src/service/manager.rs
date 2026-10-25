use super::{Layout, Platform, Scope, Spec, path_text};
use crate::{
    Result, invalid,
    runner::{Output, Runner},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Absent,
    Stopped,
    Loaded,
}
pub(super) struct Manager<'a> {
    pub layout: &'a Layout,
    pub runner: &'a dyn Runner,
}
impl Manager<'_> {
    fn target(&self, scope: &Scope) -> Result<String> {
        Ok(match self.layout.platform {
            Platform::Launchd => format!(
                "gui/{}/{}",
                rustix::process::getuid().as_raw(),
                scope.label()?
            ),
            Platform::Systemd => format!("{}.service", scope.label()?),
        })
    }
    fn call(&self, args: &[String]) -> Result<Output> {
        self.runner.run(
            match self.layout.platform {
                Platform::Launchd => "/bin/launchctl",
                Platform::Systemd => "/usr/bin/systemctl",
            },
            args,
        )
    }
    fn checked(&self, args: &[String]) -> Result<()> {
        let result = self.call(args)?;
        if result.code != 0 {
            return Err(invalid(format!(
                "Service manager command '{}' failed (exit {}): {}",
                args.join(" "),
                result.code,
                tail(&result.stderr)
            )));
        }
        Ok(())
    }
    pub fn available(&self) -> Result<bool> {
        match self.layout.platform {
            Platform::Launchd => {
                let output = self.call(&[
                    "print-disabled".into(),
                    format!("gui/{}", rustix::process::getuid().as_raw()),
                ])?;
                match output.code {
                    0 => Ok(true),
                    112 => Ok(false),
                    _ => Err(diagnostic(
                        "Cannot inspect the launchd user domain",
                        &output,
                    )),
                }
            }
            Platform::Systemd => {
                let probe =
                    match self.call(&["--user".into(), "show".into(), "--property=Version".into()])
                    {
                        Ok(output) => output,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                        Err(e) => return Err(e),
                    };
                if probe.code == 0 {
                    return Ok(true);
                }
                // A missing user bus is a known unsupported environment; other manager
                // errors are not silently converted into 'not managed'.
                let runtime = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
                    || {
                        std::path::PathBuf::from(format!(
                            "/run/user/{}",
                            rustix::process::getuid().as_raw()
                        ))
                    },
                    std::path::PathBuf::from,
                );
                if !runtime.join("bus").exists()
                    || !std::path::Path::new("/usr/bin/systemctl").exists()
                {
                    return Ok(false);
                }
                Err(diagnostic(
                    "The user bus exists but systemctl could not inspect its manager",
                    &probe,
                ))
            }
        }
    }
    pub fn state(&self, scope: &Scope) -> Result<State> {
        let target = self.target(scope)?;
        match self.layout.platform {
            Platform::Launchd => {
                let result = self.call(&["print".into(), target])?;
                match result.code {
                    0 => Ok(State::Loaded),
                    113 => Ok(State::Absent),
                    _ => Err(diagnostic("Cannot determine launchd job state", &result)),
                }
            }
            Platform::Systemd => {
                let output = self.call(&[
                    "--user".into(),
                    "show".into(),
                    target,
                    "--property=LoadState,ActiveState,FragmentPath".into(),
                ])?;
                if output.code != 0 {
                    return Err(diagnostic("Cannot determine systemd unit state", &output));
                }
                let fields: std::collections::BTreeMap<_, _> = output
                    .stdout
                    .lines()
                    .filter_map(|s| s.split_once('='))
                    .collect();
                if fields.get("LoadState") == Some(&"not-found") {
                    return Ok(State::Absent);
                }
                if fields.get("FragmentPath").copied()
                    != Some(path_text(&self.layout.unit(scope)?)?.as_str())
                {
                    return Err(invalid("Loaded systemd service has a foreign unit path."));
                }
                match fields.get("ActiveState").copied() {
                    Some("inactive" | "failed") => Ok(State::Stopped),
                    Some("active" | "activating" | "deactivating" | "reloading") => {
                        Ok(State::Loaded)
                    }
                    _ => Err(invalid("Unknown systemd unit state.")),
                }
            }
        }
    }
    pub fn install(&self, spec: &Spec) -> Result<()> {
        if self.layout.platform == Platform::Systemd {
            self.reload()?;
            self.checked(&["--user".into(), "enable".into(), path_text(&spec.unit)?])?;
        } else {
            self.checked(&["enable".into(), self.target(&spec.scope)?])?;
        }
        Ok(())
    }
    pub fn enabled(&self, scope: &Scope) -> Result<Option<bool>> {
        match self.layout.platform {
            Platform::Systemd => {
                let output =
                    self.call(&["--user".into(), "is-enabled".into(), self.target(scope)?])?;
                Ok(match (output.code, output.stdout.trim()) {
                    (0, "enabled" | "enabled-runtime") => Some(true),
                    (1, "disabled" | "masked" | "masked-runtime") => Some(false),
                    _ => None,
                })
            }
            Platform::Launchd => {
                let output = self.call(&[
                    "print-disabled".into(),
                    format!("gui/{}", rustix::process::getuid().as_raw()),
                ])?;
                if output.code != 0 {
                    return Err(diagnostic("Cannot inspect launchd enabled state", &output));
                }
                // print output is diagnostic, not an ownership/PID proof. Unknown
                // formatting stays unknown instead of granting manager authority.
                let text = output.stdout.trim();
                if !text.starts_with("disabled services = {") || !text.ends_with('}') {
                    return Ok(None);
                }
                let key = format!("\"{}\" => ", scope.label()?);
                for line in text.lines() {
                    if let Some(value) = line.trim().strip_prefix(&key) {
                        return Ok(match value {
                            "disabled" => Some(false),
                            "enabled" => Some(true),
                            _ => None,
                        });
                    }
                }
                Ok(Some(true))
            }
        }
    }
    pub fn start(&self, spec: &Spec) -> Result<()> {
        let target = self.target(&spec.scope)?;
        match self.layout.platform {
            Platform::Launchd => {
                self.checked(&["enable".into(), target.clone()])?;
                if self.state(&spec.scope)? == State::Absent {
                    self.checked(&[
                        "bootstrap".into(),
                        format!("gui/{}", rustix::process::getuid().as_raw()),
                        path_text(&spec.unit)?,
                    ])
                } else {
                    self.checked(&["kickstart".into(), target])
                }
            }
            Platform::Systemd => {
                self.checked(&["--user".into(), "reset-failed".into(), target.clone()])?;
                self.checked(&["--user".into(), "start".into(), target])
            }
        }
    }
    pub fn stop(&self, spec: &Spec) -> Result<()> {
        if self.state(&spec.scope)? != State::Loaded {
            return Ok(());
        }
        match self.layout.platform {
            Platform::Launchd => self.checked(&["bootout".into(), self.target(&spec.scope)?]),
            Platform::Systemd => {
                self.checked(&["--user".into(), "stop".into(), self.target(&spec.scope)?])
            }
        }
    }
    pub fn uninstall(&self, spec: &Spec) -> Result<()> {
        if self.layout.platform == Platform::Systemd {
            self.checked(&["--user".into(), "disable".into(), self.target(&spec.scope)?])?;
        }
        Ok(())
    }
    pub fn reload(&self) -> Result<()> {
        if self.layout.platform == Platform::Systemd {
            self.checked(&["--user".into(), "daemon-reload".into()])?;
        }
        Ok(())
    }
    pub fn login_note(&self) -> Result<Option<String>> {
        if self.layout.platform != Platform::Systemd {
            return Ok(None);
        }
        let output = self.runner.run(
            "/usr/bin/loginctl",
            &[
                "show-user".into(),
                rustix::process::getuid().as_raw().to_string(),
                "--property=Linger".into(),
            ],
        )?;
        if output.code != 0 {
            return Ok(Some(
                "Cannot determine lingering; this is a per-user login service.".into(),
            ));
        }
        Ok(match output.stdout.trim() {
            "Linger=yes" => None,
            "Linger=no" => Some(
                "Stops at logout and starts at login. For persistence without a login, \
                explicitly run: loginctl enable-linger"
                    .into(),
            ),
            _ => Some(
                "Unknown lingering state; inspect loginctl show-user --property=Linger.".into(),
            ),
        })
    }
}

fn tail(text: &str) -> &str {
    let mut start = text.len().saturating_sub(2048);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].trim()
}
fn diagnostic(message: &str, output: &Output) -> std::io::Error {
    invalid(format!(
        "{message} (exit {}): {}",
        output.code,
        tail(&output.stderr)
    ))
}
