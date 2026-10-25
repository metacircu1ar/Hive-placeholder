use super::{Context, Exit};
use clap::{Args, Subcommand};
use comb::{Result, embedding::HttpConfig, error::value, provision};
use hive_input::clap::parsers;
use std::path::PathBuf;

mod input;

#[derive(Subcommand)]
pub(super) enum ModelCommand {
    /// Serve text search only for --root. "none" is the only accepted name.
    Use {
        #[arg(value_parser = provision::input::choice)]
        name: String,
    },
    /// Validate an external embedding endpoint, then select it for --root.
    UseHttp(Http),
}
#[derive(Args)]
pub(super) struct Http {
    #[arg(long, value_parser = input::url)]
    url: String,
    #[arg(long, value_parser = input::identity)]
    model: String,
    #[arg(long, value_parser = parsers::count(HttpConfig::DIMENSIONS))]
    dimensions: usize,
    /// Pin a server-managed model revision; change it when its weights change.
    #[arg(long, default_value = "operator-managed", value_parser = input::identity)]
    revision: String,
    #[arg(long, default_value = "", value_parser = input::prefix)]
    query_prefix: String,
    #[arg(long, default_value = "", value_parser = input::prefix)]
    document_prefix: String,
    #[arg(long)]
    api_key_env: Option<String>,
    #[arg(long, default_value_t = 60,
         value_parser = parsers::unsigned(HttpConfig::TIMEOUT_SECONDS))]
    timeout_seconds: u64,
    #[arg(long)]
    request_dimensions: bool,
    #[arg(long, requires = "max_tokens")]
    tokenizer: Option<PathBuf>,
    #[arg(long, requires = "tokenizer", value_parser = parsers::count(HttpConfig::MAX_TOKENS))]
    max_tokens: Option<usize>,
}
impl From<Http> for HttpConfig {
    fn from(h: Http) -> Self {
        Self {
            url: h.url,
            model: h.model,
            dimensions: h.dimensions,
            revision: h.revision,
            query_prefix: h.query_prefix,
            document_prefix: h.document_prefix,
            api_key_env: h.api_key_env,
            timeout_seconds: h.timeout_seconds,
            request_dimensions: h.request_dimensions,
            tokenizer: h.tokenizer,
            max_tokens: h.max_tokens,
        }
    }
}
pub(super) fn execute(context: &Context, command: ModelCommand) -> Result<Exit> {
    let json = match command {
        ModelCommand::Use { name: _ } => value(comb::services::paused(&context.comb, || {
            provision::select_none(&context.comb)
        })?)?,
        ModelCommand::UseHttp(args) => {
            let config: HttpConfig = args.into();
            comb::services::validate_environment(
                &context.comb,
                &config.api_key_env.clone().into_iter().collect::<Vec<_>>(),
            )?;
            // Validate URL, tokenizer and credentials before stopping the old service.
            provision::validate_http(config.clone())?;
            value(comb::services::paused(&context.comb, || {
                provision::use_http(&context.comb, config)
            })?)?
        }
    };
    Ok(Exit::Json(json))
}
