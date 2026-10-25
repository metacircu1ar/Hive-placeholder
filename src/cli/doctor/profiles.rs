//! Native auth probes and the provider policies actually enforced by `Hive`.
use super::details;
use super::{Cli, Config, Path, Report, Status, api, command, quote};

pub(super) fn profiles(
    cli: &Cli,
    config_path: &Path,
    root: &Path,
    config: &Config,
    report: &mut Report,
) {
    let auth = super::super::auth::authentication(
        &super::super::AuthCommand::List,
        config_path,
        root.into(),
        cli,
    );
    let auth = match auth {
        Ok(auth) => auth,
        Err(error) => {
            report.add("authentication", Status::Error, api::value(error), vec![]);
            return;
        }
    };
    if config.auth_profiles.is_empty() {
        report.add(
            "authentication",
            Status::Error,
            api::value("No login profiles configured."),
            vec![format!(
                "{} setup",
                command(cli, config_path, root, "setup")
            )],
        );
    }
    for name in config.auth_profiles.keys() {
        profile(cli, config_path, root, name, &auth, report);
    }
}

fn provider_policy(name: &str, result: &crate::auth::Report, report: &mut Report) {
    use crate::provider_support::Support;
    let support = result
        .provider_support
        .as_ref()
        .map(|observation| observation.support);
    report.add(
        format!("provider:{name}"),
        match support {
            Some(Support::Tested) => Status::Ok,
            Some(Support::Unsupported) => Status::Error,
            _ => Status::Warning,
        },
        api::value(details::ProviderPolicy {
            registry: &result.provider_registry,
            observation: &result.provider_support,
        }),
        if support == Some(Support::Unsupported) {
            vec![format!(
                "Update the selected provider to at least {} and restart the daemon.",
                result.provider_registry.minimum
            )]
        } else {
            vec![]
        },
    );
}

fn profile(
    cli: &Cli,
    config_path: &Path,
    root: &Path,
    name: &str,
    auth: &crate::auth::Auth,
    report: &mut Report,
) {
    let fixes = vec![format!(
        "{} auth status {}",
        command(cli, config_path, root, "auth status"),
        quote(name)
    )];
    match auth.status(name) {
        Ok(result) => {
            use crate::auth::Status as AuthStatus;
            let status = match result.state {
                AuthStatus::Authenticated => Status::Ok,
                AuthStatus::ProfileBusy => Status::Warning,
                _ => Status::Error,
            };
            let fix = match result.state {
                AuthStatus::Missing | AuthStatus::ReauthenticationRequired => vec![format!(
                    "{} auth login {}",
                    command(cli, config_path, root, "auth login"),
                    quote(name)
                )],
                AuthStatus::RecoveryRequired => vec![format!(
                    "{} auth recover {}",
                    command(cli, config_path, root, "auth recover"),
                    quote(name)
                )],
                _ => fixes,
            };
            // Deliberately omit identity, raw provider output and credential contents.
            report.add(
                format!("auth:{name}"),
                status,
                api::value(details::Login {
                    provider: result.provider,
                    state: result.state,
                    provider_available: result.provider_available,
                    executable: &result.provider_executable,
                    version: &result.provider_version,
                    diagnostic: &result.diagnostic,
                    warnings: &result.warnings,
                }),
                fix,
            );
            provider_policy(name, &result, report);
        }
        Err(error) => report.add(
            format!("auth:{name}"),
            Status::Error,
            api::value(error),
            fixes,
        ),
    }
}
