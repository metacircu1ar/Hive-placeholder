//! Aggregate diagnostics. A broken component must not hide the remaining checks.
mod comb;
#[cfg(all(test, feature = "comb"))]
#[allow(clippy::unwrap_used)]
mod comb_tests;
mod details;
mod parts;
mod profiles;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

use super::{Cli, CommandOutcome, Config, Path, api, runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

#[derive(Deserialize, Serialize)]
pub(super) struct Check {
    pub(super) name: String,
    pub(super) status: Status,
    pub(super) detail: Value,
    fixes: Vec<String>,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Ok,
    Warning,
    Error,
    Info,
}
#[derive(Default, Deserialize, Serialize)]
pub(super) struct Report {
    pub(super) checks: Vec<Check>,
}
impl Report {
    fn add(&mut self, name: impl Into<String>, status: Status, detail: Value, fixes: Vec<String>) {
        self.checks.push(Check {
            name: name.into(),
            status,
            detail,
            fixes,
        });
    }
    fn finish(self) -> CommandOutcome {
        let errors = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Error)
            .count();
        let warnings = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Warning)
            .count();
        if let Err(error) = self.print(&mut std::io::stderr().lock(), errors, warnings) {
            tracing::warn!(name: "doctor.output_failed",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "preserve_report");
        }
        CommandOutcome {
            input: None,
            value: api::value(self),
            success: errors == 0,
        }
    }
    fn print(
        &self,
        diagnostics: &mut impl std::io::Write,
        errors: usize,
        warnings: usize,
    ) -> std::io::Result<()> {
        writeln!(
            diagnostics,
            "Hive doctor: {errors} failed checks, {warnings} warnings."
        )?;
        for check in &self.checks {
            if matches!(check.status, Status::Error | Status::Warning) {
                writeln!(diagnostics, "  {}: {}", check.name, check.detail)?;
                for fix in &check.fixes {
                    writeln!(diagnostics, "    {fix}")?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn run(cli: &Cli, config_path: &Path, root: &Path) -> CommandOutcome {
    let mut report = Report::default();
    report.add("hive", Status::Ok, super::version::identity(), vec![]);
    let config = match Config::load(config_path) {
        Ok(config) => {
            report.add(
                "config",
                Status::Ok,
                api::value(details::PathDetail { path: config_path }),
                vec![],
            );
            Some(config)
        }
        Err(error) => {
            report.add(
                "config",
                Status::Error,
                api::value(error),
                vec![format!(
                    "{} setup",
                    command(cli, config_path, root, "setup")
                )],
            );
            None
        }
    };
    parts::installed(
        cli,
        &mut report,
        config
            .as_ref()
            .is_some_and(|c| !comb::bindings(c).is_empty()),
    );
    if let Some(config) = &config {
        sandbox_settings(config, &mut report);
        profiles::profiles(cli, config_path, root, config, &mut report);
        skills(config, &mut report);
        comb::check(config, &mut report);
    }
    daemon(cli, config_path, root, config.as_ref(), &mut report);
    parts::skills(&mut report);
    match super::services::status(config_path, root) {
        Ok(state) => {
            let healthy = state.ready
                && state.missing_environment.is_empty()
                && state.warnings.is_empty()
                && state.enabled == Some(true);
            let status = if !state.managed {
                Status::Info
            } else if healthy {
                Status::Ok
            } else {
                Status::Error
            };
            report.add(
                "services",
                status,
                api::value(state),
                if healthy {
                    vec![]
                } else {
                    vec![format!(
                        "{} service status",
                        command(cli, config_path, root, "service status")
                    )]
                },
            );
        }
        Err(error) => report.add(
            "services",
            Status::Error,
            api::value(error),
            vec![format!(
                "{} service status",
                command(cli, config_path, root, "service status")
            )],
        ),
    }
    report.finish()
}

fn sandbox_settings(config: &Config, report: &mut Report) {
    let roles: Vec<_> = config
        .roles
        .iter()
        .filter(|(_, role)| role.sandbox == crate::config::Sandbox::FullAccess)
        .map(|(name, _)| name)
        .collect();
    report.add(
        "sandbox",
        if roles.is_empty() && !config.runtime.allow_spawn_widening {
            Status::Ok
        } else {
            Status::Warning
        },
        api::value(details::SandboxPolicy {
            full_access_roles: &roles,
            allow_spawn_widening: config.runtime.allow_spawn_widening,
        }),
        vec![],
    );
}

#[derive(Deserialize)]
struct DaemonIdentity {
    configuration: Option<crate::config::ConfigurationIdentity>,
    product_version: String,
    compiled_features: Vec<String>,
    status: String,
}
fn daemon(cli: &Cli, config: &Path, root: &Path, loaded: Option<&Config>, report: &mut Report) {
    let prefix = |path| command(cli, config, root, path);
    let result = crate::calls::executor()
        .map_err(crate::calls::ObservationError::from)
        .and_then(|rt| {
            rt.block_on(async {
                runtime::client::Client::new(root)
                    .observe::<DaemonIdentity>(
                        "doctor",
                        &super::Request::Ping,
                        tokio::time::Instant::now()
                            + std::time::Duration::from_secs(cli.probe_timeout),
                    )
                    .await
            })
        });
    match result {
        Ok(identity) => {
            let same = identity.product_version == env!("CARGO_PKG_VERSION")
                && identity.compiled_features == runtime::features::COMPILED
                && identity.status == "running";
            // Reuse the config check above; its failure is already in the report.
            let current = loaded.and_then(|c| c.loaded.as_ref());
            if current.is_some() && current != identity.configuration.as_ref() {
                report.add(
                    "loaded_configuration",
                    Status::Warning,
                    api::value(details::ConfigurationChange {
                        restart_pending: true,
                        loaded: &identity.configuration,
                        current,
                    }),
                    vec![format!("{} setup --restart-daemon", prefix("setup"))],
                );
            }
            report.add(
                "daemon",
                if same { Status::Ok } else { Status::Error },
                api::value(details::DaemonDetail {
                    product_version: &identity.product_version,
                    compiled_features: &identity.compiled_features,
                    status: &identity.status,
                }),
                if same {
                    vec![]
                } else {
                    vec![
                        format!("{} daemon stop", prefix("daemon stop")),
                        format!("{} daemon start", prefix("daemon start")),
                    ]
                },
            );
        }
        Err(error) => report.add(
            "daemon",
            Status::Error,
            api::value(error),
            vec![format!("{} daemon start", prefix("daemon start"))],
        ),
    }
}

fn skills(config: &Config, report: &mut Report) {
    for (name, role) in &config.roles {
        if let Some(path) = &role.instructions {
            report.add(
                format!("instructions:{name}"),
                if path.is_file() {
                    Status::Ok
                } else {
                    Status::Error
                },
                api::value(details::PathDetail { path }),
                vec![],
            );
        }
    }
}

pub(super) fn command(cli: &Cli, config: &Path, root: &Path, target: &str) -> String {
    let mut prefix = "hivectl".to_owned();
    for (id, flag, value) in [
        ("config", "--config", config.as_os_str()),
        ("state_dir", "--state-dir", root.as_os_str()),
        ("codex_bin", "--codex-bin", cli.codex_bin.as_os_str()),
        ("claude_bin", "--claude-bin", cli.claude_bin.as_os_str()),
    ] {
        if super::parsing::globals::consumes(target, id) {
            prefix.push(' ');
            prefix.push_str(flag);
            prefix.push(' ');
            prefix.push_str(&quote(&value.to_string_lossy()));
        }
    }
    prefix
}
pub(super) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
