use super::{
    Command, Error, Layout, Path, Result, files, health, owned, paused, provision,
    required_environment, run_new, scope, status,
};
use crate::setup::{Options, Outcome, Plan, Prepared};

pub(super) fn execute(root: &Path, socket: Option<&Path>, input: Prepared) -> Result<Outcome> {
    let options = input.options();
    let plan = Plan::inspect_prepared(root, socket, &input)?;
    preflight_service(&plan, options)?;
    if options.dry_run {
        let mut result = crate::setup::apply(root, socket, &input)?;
        result.service = if options.no_services {
            "not requested"
        } else {
            "planned: install and start user service"
        };
        return Ok(result);
    }
    let live = health(&plan.root, true)?;
    let unchanged = if let Some(live) = &live {
        !plan.recover_activation
            && !plan.text_only
            && provision::selection_matches(
                &plan.root,
                plan.http.as_ref(),
                live.model.as_ref().map(|m| m.space_id.as_str()),
            )?
    } else {
        false
    };
    let mut outcome = if unchanged && crate::setup::bridge_present(&plan)? {
        Outcome {
            changed: false,
            dry_run: false,
            backfill: if live.as_ref().is_some_and(|live| live.model.is_some()) {
                "background indexing"
            } else {
                "disabled; text search only"
            },
            plan: plan.clone(),
            service: "running",
        }
    } else {
        // Complete endpoint validation while the old service still serves.
        if !unchanged
            && let Some(http) = &plan.http
            && needs_http_probe(&plan, http)?
        {
            provision::validate_http(http.clone())?;
        }
        let apply = || crate::setup::apply(root, socket, &input);
        if owned(&plan.root)? {
            paused(&plan.root, apply)?
        } else {
            apply()?
        }
    };
    configure_service(&plan, options, &mut outcome)?;
    Ok(outcome)
}

fn preflight_service(plan: &Plan, options: &Options) -> Result<()> {
    if options.no_services {
        return Ok(());
    }
    if !status(&plan.root)?.manager_available {
        return Ok(());
    }
    let layout = Layout::discover()?;
    let installed = hive_setup_support::service::load(&layout, &scope(&plan.root)?)?;
    let environment = options
        .service_environment
        .as_deref()
        .map(files::absolute)
        .transpose()?;
    if let Some(spec) = &installed
        && environment.is_some()
        && environment != spec.environment_file
    {
        return Err(Error::service_environment_conflict());
    }
    if let Some(path) = &options.service_path {
        let path = hive_setup_support::environment::service_path(Some(path))?;
        if let Some(spec) = &installed
            && spec.environment.get("PATH") != Some(&path)
        {
            return Err(Error::service_path_conflict());
        }
    }
    let path = environment.as_deref().or_else(|| {
        installed
            .as_ref()
            .and_then(|s| s.environment_file.as_deref())
    });
    let names = hive_setup_support::environment::required(if let Some(http) = &plan.http {
        http.api_key_env.clone().into_iter().collect()
    } else if plan.root.join(".comb").exists() {
        required_environment(&plan.root)?
    } else {
        vec![]
    });
    let missing = hive_setup_support::environment::missing(path, &names)?;
    if !missing.is_empty() {
        return Err(Error::service_environment_required(&missing));
    }
    Ok(())
}

fn configure_service(plan: &Plan, options: &Options, outcome: &mut Outcome) -> Result<()> {
    if !options.no_services {
        let state = status(&plan.root)?;
        if !state.manager_available {
            outcome.service = "not managed; run comb --root ROOT serve in the foreground";
            return Ok(());
        }
        if !state.managed {
            run_new(
                &plan.root,
                &Command::Install {
                    environment_file: options.service_environment.clone(),
                    program: None,
                    service_path: options.service_path.clone(),
                },
            )?;
            outcome.changed = true;
        } else if let Some(environment) = &options.service_environment {
            let layout = Layout::discover()?;
            let spec = hive_setup_support::service::load(&layout, &scope(&plan.root)?)?
                .ok_or_else(Error::service_disappeared)?;
            if spec.environment_file.as_deref() != Some(files::absolute(environment)?.as_path()) {
                return Err(Error::service_environment_conflict());
            }
        }
        if !state.ready {
            use hive_setup_support::service::StartDecision;
            match super::start_for_setup(
                &plan.root,
                options.preserve_stopped_services && state.managed,
            )? {
                StartDecision::PreservedStopped => {
                    outcome.service = "stopped; retained";
                    return Ok(());
                }
                StartDecision::AlreadyLoaded => {
                    outcome.service = "starting";
                    return Ok(());
                }
                StartDecision::Started => outcome.changed = true,
            }
        }
        outcome.service = "running";
    }
    Ok(())
}

fn needs_http_probe(plan: &Plan, http: &crate::embedding::HttpConfig) -> Result<bool> {
    if plan.initialize || plan.recover_activation {
        return Ok(true);
    }
    Ok(
        !matches!(provision::settings(&plan.root)?.and_then(|s|s.embeddings),
            Some(crate::embedding::Config::Http(saved)) if saved==*http),
    )
}
