//! Execution for this component.
use super::api;
use super::{
    Cli, Command, CommandOutcome, EXIT_ERROR, Error, Result, attach, dispatch, execute, live,
    logging, logs, panes, parsing, read, services, tui, version,
};

pub(super) fn run_parsed(cli: Cli) {
    if let Command::Version { json } = &cli.command {
        std::process::exit(version::run(*json));
    }
    if let Command::Logs(args) = &cli.command {
        std::process::exit(logs::run(&cli, args));
    }
    if let Command::Read(args) = &cli.command
        && args.follow
    {
        std::process::exit(read::run(&cli, args));
    }
    if let Command::Live(args) = &cli.command
        && args.follow
    {
        std::process::exit(live::run(&cli, args));
    }
    if let Command::Attach(args) = &cli.command {
        std::process::exit(attach::run(&cli, args));
    }
    if let Command::Panes(args) = &cli.command {
        std::process::exit(panes::run(&cli, args));
    }
    if let Command::Tui(args) = &cli.command {
        std::process::exit(tui::run(&cli, args));
    }
    if matches!(cli.command, Command::Mcp) {
        let result = logging::run(|| serve_mcp(&cli));
        if let Err(error) = result {
            std::process::exit(parsing::emit_stderr(parsing::human(&error)));
        }
        return;
    }
    let prepared = match dispatch::Prepared::new(cli) {
        Ok(prepared) => prepared,
        Err(failure) => std::process::exit(parsing::emit_file_failure(&failure)),
    };
    let mut spec = None;
    let result = execute(prepared, &mut spec);
    let (value, code, human, suppressed) = match result {
        Ok(CommandOutcome {
            input: Some(input), ..
        }) => std::process::exit(input.emit()),
        Ok(CommandOutcome {
            value,
            success,
            input: None,
        }) => (value, i32::from(!success), String::new(), false),
        Err(error @ Error::Input { .. }) => {
            let value = api::value(api::ErrorResponse {
                error: error.clone(),
            });
            let Error::Input { input, .. } = error else {
                unreachable!()
            };
            std::process::exit(parsing::emit_remote_input(&input, &value, None));
        }
        Err(error) => {
            let suppressed = services::suppress(spec.as_ref(), &error);
            let human = parsing::human(&error);
            (
                api::value(api::ErrorResponse { error }),
                EXIT_ERROR,
                human,
                suppressed,
            )
        }
    };
    std::process::exit(parsing::emit_command(&value, code, human, suppressed));
}

pub(super) fn serve_mcp(cli: &Cli) -> Result<()> {
    let root = cli.state_root()?;
    let executor = crate::calls::executor()?;
    let result = executor.block_on(crate::external_mcp::serve(root));
    // Tokio stdio uses blocking workers. An unread stdout must not make
    // Runtime::drop wait forever after EOF or transport overload.
    executor.shutdown_background();
    result
}
