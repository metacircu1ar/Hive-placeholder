//! Guided provisioning using existing native authentication and product CLIs.
mod comb;
mod config;
pub(super) mod daemon;
mod options;
pub(super) mod parts;
mod plan;
mod provider;
pub(super) mod publication;
mod reports;
pub(super) mod skills;
mod terminal;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod uninstall;
use super::{Error, Result};
pub(super) use options::Options;
fn invalid(message: impl Into<String>) -> Error {
    Error::ConfigInvalid {
        message: message.into().into(),
        source: None,
    }
}
fn io(error: std::io::Error) -> Error {
    super::services::error(error)
}

use super::api;
use super::{Cli, CommandOutcome};
use crate::config::Config;
use std::path::Path;

pub(super) fn run(
    cli: &Cli,
    config_path: &Path,
    root: &Path,
    args: &Options,
) -> Result<CommandOutcome> {
    let resolved = args.resolve()?;
    let options = &resolved.values;
    if options.uninstall == Some(true) {
        super::parsing::globals::removal(&cli.explicit_globals, options.purge == Some(true))?;
        return uninstall::run(config_path, root, options);
    }
    configure(cli, config_path, root, options).map_err(|error| resolved.attribute(error))
}
fn configure(
    cli: &Cli,
    config_path: &Path,
    root: &Path,
    options: &options::Values,
) -> Result<CommandOutcome> {
    let snapshot = publication::Snapshot::inspect(config_path)?;
    let mut terminal = terminal::Terminal::open(options.yes == Some(true));
    let mut plan = plan::Plan::inspect(&snapshot, options, &mut terminal)?;
    let skill_dir = parts::skills(options.skills_dir.as_deref())?;
    let context = Context {
        cli,
        config_path,
        root,
        options,
    };
    let PreparedParts {
        comb,
        copies,
        preflight,
    } = prepare_parts(context, &snapshot, &mut plan, &skill_dir)?;
    if options.dry_run == Some(true) {
        return Ok(preview(
            &snapshot,
            root,
            &plan,
            comb.as_ref(),
            &copies,
            options,
            preflight,
        ));
    }
    let applied = apply_setup(context, &snapshot, &mut plan, comb.as_ref(), &copies)?;
    let mut pending = applied.pending;
    let restart_pending = pending_steps(
        cli,
        config_path,
        root,
        comb.as_ref(),
        applied.comb_result.as_ref(),
        &mut pending,
    )?;
    let command = |path| super::doctor::command(cli, config_path, root, path);
    let doctor = super::doctor::run(cli, config_path, root);
    let ready = doctor.success && pending.is_empty() && !restart_pending;
    let stopped = applied
        .comb_result
        .as_ref()
        .filter(|r| r.pending_service())
        .and(comb.as_ref())
        .and_then(|e| e.details.bridge_file.as_deref());
    let success = accepted_checks(&doctor, &plan, options, applied.manual, stopped)?;
    Ok(CommandOutcome {
        input: None,
        success,
        value: api::value(reports::Applied {
            readiness: reports::Readiness {
                configured: true,
                ready,
            },
            publication: reports::Publication {
                changed: applied.changed,
                backup: applied.backup,
                restart_pending,
                daemon_restarted: applied.restarted,
            },
            providers: plan.providers,
            comb: applied.comb_result,
            pending,
            doctor: doctor.value,
            first_spawn: format!(
                "{} spawn --cwd . --body 'Describe the current repository.'",
                command("spawn")
            ),
            guide: skill_dir.parent().map(|p| p.join("docs/setup.md")),
        }),
    })
}

fn accepted_checks(
    doctor: &CommandOutcome,
    plan: &plan::Plan,
    options: &options::Values,
    manual_services: bool,
    comb_stopped: Option<&Path>,
) -> Result<bool> {
    let checks: super::doctor::Report = serde_json::from_value(doctor.value.clone())
        .map_err(|cause| invalid("Cannot decode doctor checks.").with_source(cause))?;
    Ok(checks
        .checks
        .iter()
        .filter(|c| c.status == super::doctor::Status::Error)
        .all(|c| expected_pending(c, plan, options, manual_services, comb_stopped)))
}
#[derive(serde::Deserialize)]
struct Failure {
    code: PendingFailure,
}
#[derive(serde::Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PendingFailure {
    DaemonUnavailable,
    CombUnavailable,
}
#[derive(serde::Deserialize)]
struct NestedFailure {
    error: Failure,
    credential_file: std::path::PathBuf,
}
#[derive(serde::Deserialize)]
struct Authentication {
    state: crate::auth::Status,
}
fn expected_pending(
    check: &super::doctor::Check,
    plan: &plan::Plan,
    options: &options::Values,
    manual_services: bool,
    comb_stopped: Option<&Path>,
) -> bool {
    if options.no_services == Some(true) || manual_services {
        if check.name == "daemon"
            && serde_json::from_value::<Failure>(check.detail.clone())
                .is_ok_and(|e| e.code == PendingFailure::DaemonUnavailable)
        {
            return true;
        }
        if check.name == "services"
            && serde_json::from_value::<StoppedService>(check.detail.clone()).is_ok_and(|s| {
                !s.ready
                    && s.missing_environment.is_empty()
                    && s.warnings.is_empty()
                    && s.failure.is_none()
            })
        {
            return true;
        }
    }
    if check.name.starts_with("comb:")
        && serde_json::from_value::<NestedFailure>(check.detail.clone()).is_ok_and(|e| {
            e.error.code == PendingFailure::CombUnavailable
                && (options.no_services == Some(true)
                    || comb_stopped == Some(e.credential_file.as_path()))
        })
    {
        return true;
    }
    plan.login == options::Login::Skip
        && plan
            .providers
            .iter()
            .any(|p| !p.verified && check.name == format!("auth:{}", p.profile))
        && serde_json::from_value::<Authentication>(check.detail.clone()).is_ok_and(|a| {
            matches!(
                a.state,
                crate::auth::Status::Missing
                    | crate::auth::Status::ReauthenticationRequired
                    | crate::auth::Status::PresentUnverified
            )
        })
}

#[derive(serde::Deserialize)]
struct StoppedService {
    ready: bool,
    missing_environment: Vec<String>,
    warnings: Vec<String>,
    failure: Option<serde_json::Value>,
}

fn pending_steps(
    cli: &Cli,
    config_path: &Path,
    root: &Path,
    comb: Option<&comb::Comb>,
    comb_result: Option<&comb::Applied>,
    pending: &mut Vec<String>,
) -> Result<bool> {
    let loaded = daemon::observe(root)?;
    let current = crate::config::Config::load(config_path)?.loaded;
    let restart_pending = loaded.as_ref().is_some_and(|l| l.configuration != current);
    let command = |path| super::doctor::command(cli, config_path, root, path);
    if restart_pending {
        pending.push(format!(
            "Configuration is saved but not loaded: {} setup --restart-daemon; for a \
            manual daemon use {} daemon stop, then {} daemon start.",
            command("setup"),
            command("daemon stop"),
            command("daemon start")
        ));
    }
    if let Some(result) = comb_result
        && result.service != "running"
        && let Some(comb) = comb
    {
        pending.push(format!(
            "{} --root {} serve",
            super::doctor::quote(&comb.binary.to_string_lossy()),
            super::doctor::quote(&comb.details.root.to_string_lossy())
        ));
    }
    Ok(restart_pending)
}

fn preview(
    snapshot: &publication::Snapshot,
    root: &Path,
    plan: &plan::Plan,
    comb: Option<&comb::Comb>,
    copies: &skills::Copies,
    options: &options::Values,
    preflight: String,
) -> CommandOutcome {
    CommandOutcome::ok(api::value(reports::Preview {
        dry_run: true,
        config: snapshot.target(),
        state: root,
        providers: &plan.providers,
        proposed_config_before_model_discovery: preflight,
        deferred: "Native authentication and model/effort discovery run only when applying setup.",
        comb: comb.map(|e| &e.details),
        skills: copies.paths(),
        services: options.no_services != Some(true),
        restart_daemon: options.restart_daemon == Some(true),
    }))
}

struct PreparedParts {
    comb: Option<comb::Comb>,
    copies: skills::Copies,
    preflight: String,
}
fn prepare_parts(
    context: Context<'_>,
    snapshot: &publication::Snapshot,
    plan: &mut plan::Plan,
    skill_dir: &Path,
) -> Result<PreparedParts> {
    let Context {
        cli,
        config_path,
        root,
        options,
    } = context;
    let existing = if snapshot.text.is_empty() {
        None
    } else {
        Some(Config::parse_at(&snapshot.text, snapshot.target())?)
    };
    let comb = comb::Comb::inspect(
        options,
        existing.as_ref(),
        skill_dir,
        plan,
        cli.probe_timeout,
    )?;
    daemon::preflight(config_path, root, options)?;
    let copies = skills::Copies::inspect(
        skill_dir,
        options.user_skills.as_deref().unwrap_or_default(),
    )?;
    let preflight = config::patch(&snapshot.text, &plan.additions)?;
    Ok(PreparedParts {
        comb,
        copies,
        preflight,
    })
}

#[derive(Clone, Copy)]
struct Context<'a> {
    cli: &'a Cli,
    config_path: &'a Path,
    root: &'a Path,
    options: &'a options::Values,
}
struct AppliedSetup {
    pending: Vec<String>,
    comb_result: Option<comb::Applied>,
    changed: bool,
    backup: Option<std::path::PathBuf>,
    restarted: bool,
    manual: bool,
}
fn apply_setup(
    context: Context<'_>,
    snapshot: &publication::Snapshot,
    plan: &mut plan::Plan,
    comb: Option<&comb::Comb>,
    copies: &skills::Copies,
) -> Result<AppliedSetup> {
    let Context {
        cli,
        config_path,
        root,
        options,
    } = context;
    let mut pending = provider::configure(cli, root, snapshot, plan, options)?;
    let text = config::patch(&snapshot.text, &plan.additions)?;
    Config::parse_at(&text, snapshot.target())?;
    let comb_result = comb.map(comb::Comb::apply).transpose()?;
    let skills_changed = copies.apply()?;
    let mut changed =
        text != snapshot.text || skills_changed || comb_result.as_ref().is_some_and(|e| e.changed);
    let daemon::Publication { backup, restarted } =
        daemon::publish(config_path, root, snapshot, &text, options)?;
    changed |= restarted;
    if backup.is_some() {
        eprintln!("Existing configuration backed up; published configuration is mode 0600.");
    }
    let services = daemon::services(cli, config_path, root, options)?;
    changed |= services.changed;
    pending.extend(services.pending);
    Ok(AppliedSetup {
        pending,
        comb_result,
        changed,
        backup,
        restarted,
        manual: services.manual,
    })
}
