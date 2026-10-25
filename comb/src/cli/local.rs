use super::{Context, Exit, args, command::LocalCommand, limits, runtime};
use comb::{
    Comb, Result,
    embedding::{self, Config, Embedder, Engine},
    error::value,
    service::{self, auth::Connection},
};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct Initialized {
    comb_id: comb::values::CombId,
    stats: comb::store::Stats,
}
#[derive(Serialize)]
struct Stopped {
    stopped: bool,
}

impl LocalCommand {
    pub(super) fn execute(self, context: &Context) -> Result<Exit> {
        match self {
            Self::Logs(args) => super::logs::run(context, args),
            Self::SetupMaintenance(options) => {
                Ok(Exit::Json(comb::maintenance::run(&context.comb, &options)?))
            }
            Self::Service(command, logging) => Ok(Exit::Json(value(
                comb::services::run_configured(&context.comb, &command, &logging)?,
            )?)),
            Self::Setup(options) => Ok(Exit::Json(value(
                options.run(&context.comb, context.socket.as_deref())?,
            )?)),
            Self::Model(command) => super::models::execute(context, *command),
            Self::Version { json } => version(json),
            Self::Init { dir } => init(context, dir.as_deref()),
            Self::Upgrade { dry_run } => {
                Ok(Exit::Json(value(Comb::upgrade(&context.comb, dry_run)?)?))
            }
            Self::Serve { plan, args, .. } => {
                // Managed environment may override the level; validate before recovery.
                let config = args.logging.backend()?;
                let prepared = super::logging::preparation(config.clone(), || {
                    plan.ok_or_else(comb::Error::internal_serialization)?
                        .prepare()
                })?;
                serve(prepared, config)
            }
            Self::Mcp { connection } => mcp(connection),
            Self::Embed { args, config } => embed(context, args, config),
            Self::Rebuild => Ok(Exit::Json(value(Comb::rebuild(&context.comb)?.stats()?)?)),
        }
    }
}

fn version(json: bool) -> Result<Exit> {
    let info = comb::build_info();
    if json {
        Ok(Exit::Json(value(info)?))
    } else {
        Ok(Exit::Text(format!(
            "{} {} ({})\n",
            info.product,
            info.product_version,
            info.compiled_features.join(",")
        )))
    }
}

fn init(context: &Context, dir: Option<&Path>) -> Result<Exit> {
    let comb = Comb::init(dir.unwrap_or(&context.comb))?;
    Ok(Exit::Json(value(Initialized {
        comb_id: comb.identity(),
        stats: comb.stats()?,
    })?))
}

fn serve(prepared: service::Prepared, config: hive_log::Config) -> Result<Exit> {
    let logging = super::serve_log::ProcessLog::start(&prepared, config)?;
    logging.finish(run_prepared(prepared))
}

fn run_prepared(prepared: service::Prepared) -> Result<Exit> {
    let rt = runtime()?;
    // Keep long preparation interruptible, then install before advertising readiness.
    let mut interrupt = {
        let _entered = rt.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?
    };
    let mut terminate = {
        let _entered = rt.enter();
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
    };
    let server = prepared.bind()?;
    tracing::info!(name: "comb.ready", success = true);
    hive_setup_support::managed::ready();
    let result = rt.block_on(server.run(async {
        let signal = tokio::select! { _ = interrupt.recv() => libc::SIGINT,
        _ = terminate.recv() => libc::SIGTERM };
        tracing::info!(name: "comb.shutdown_signal",
             signal = signal,
             decision = "graceful_shutdown");
    }));
    rt.shutdown_timeout(limits::SHUTDOWN);
    result?;
    Ok(Exit::Json(value(Stopped { stopped: true })?))
}

fn mcp(connection: Connection) -> Result<Exit> {
    let rt = runtime()?;
    let result = rt.block_on(service::mcp::run(
        connection,
        tokio::io::BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    ));
    rt.shutdown_timeout(limits::SHUTDOWN);
    result?;
    // MCP stdout consists exclusively of JSON-RPC frames.
    Ok(Exit::Silent)
}

fn embed(context: &Context, args: args::Embed, config: Config) -> Result<Exit> {
    let mut engine = Engine::load(config)?;
    let mut comb = Comb::open_for_embedding(&context.comb, engine.space(), args.replace)?;
    comb.activate_engine(&mut engine, args.replace)?;
    if args.retry_failed {
        comb.retry_embedding_failures()?;
    }
    let mut backfill = embedding::Backfill::default();
    while backfill.offline_step(&mut comb, &mut engine)? {}
    Ok(Exit::Json(value(
        comb.index()?
            .embedding_coverage(comb::search::Filter::default())?,
    )?))
}
