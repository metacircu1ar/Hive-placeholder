//! Daemon response validation and provider model discovery.
mod input_context;
use super::{CommandOutcome, Config, Path, PathBuf, Request, Result, State, api, runtime};
use crate::runtime::api::InputContext as Op;

pub(super) fn remote_response(
    root: &Path,
    request_id: &str,
    request: Request,
) -> Result<CommandOutcome> {
    let value = runtime::client(root, request_id, &request)?;
    let reply = api::DaemonReply::new(&value)?;
    let input = input_reply(&reply, &request)?;
    let ok = !reply.has_error();
    if ok {
        match &request {
            Request::Status(_) => {
                reply.agent()?;
            }
            Request::Read(args)
                if !args
                    .target
                    .required(Op::Read, "/target")?
                    .starts_with("artifact:")
                    && !args
                        .target
                        .required(Op::Read, "/target")?
                        .starts_with("join:") =>
            {
                reply.events()?;
            }
            _ => {}
        }
    }
    let value = input.as_ref().map_or(value, InputReply::value);
    Ok(CommandOutcome {
        value,
        success: ok,
        input,
    })
}

pub(super) fn models_command(
    config_path: &Path,
    root: PathBuf,
    auth: &str,
    binaries: &runtime::Binaries,
) -> Result<CommandOutcome> {
    let config = Config::load(config_path)?;
    let profile = super::auth::selection::profile(&config, auth, "models", "--auth")?;
    super::auth::selection::models(profile)?;
    let binary = binaries.discovery(profile.provider)?;
    let models = runtime::models(&config, &State::new(root)?, auth, &binary)?;
    Ok(CommandOutcome::ok(api::value(api::ModelsResponse {
        models,
    })))
}

/// A direct command rejection, never an accepted call's nested observation error.
pub(super) struct InputReply {
    error: hive_input::InputError,
    request_id: Option<String>,
    path: Option<PathBuf>,
}
impl InputReply {
    pub(super) fn file_source(
        mut self,
        command: &str,
        inline_flag: &str,
        file_flag: &str,
        path: Option<&Path>,
    ) -> Result<Self> {
        if self.error.field() == inline_flag
            && let Some(path) = path
        {
            self.error = hive_input::InputError::new(
                self.error.code(),
                self.error.message(),
                file_flag,
                self.error.expected(),
                super::catalog::examples(command)
                    .ok_or_else(runtime::client::envelope::invalid)?
                    .as_slice()
                    .to_vec(),
            )
            .map_err(|cause| runtime::client::envelope::invalid().with_source(cause))?;
            self.path = Some(path.to_owned());
        }
        Ok(self)
    }

    pub(super) fn for_wait(mut self) -> Result<Self> {
        let field = match self.error.field() {
            "TARGET" => "TARGET",
            "--after" => "--after",
            _ => return Err(runtime::client::envelope::invalid()),
        };
        self.error = hive_input::InputError::new(
            self.error.code(),
            self.error.message(),
            field,
            self.error.expected(),
            super::catalog::examples("wait")
                .ok_or_else(runtime::client::envelope::invalid)?
                .as_slice()
                .to_vec(),
        )
        .map_err(|cause| runtime::client::envelope::invalid().with_source(cause))?;
        Ok(self)
    }
    pub(super) fn value(&self) -> serde_json::Value {
        #[derive(serde::Serialize)]
        struct Envelope<'a> {
            error: hive_input::WireError<'a>,
            #[serde(skip_serializing_if = "Option::is_none")]
            request_id: Option<&'a str>,
        }
        api::value(Envelope {
            error: self.error.wire(),
            request_id: self.request_id.as_deref(),
        })
    }
    pub(super) fn emit(&self) -> i32 {
        super::parsing::emit_remote_input(&self.error, &self.value(), self.path.as_deref())
    }
}

pub(super) fn input_reply(
    reply: &api::DaemonReply<'_>,
    request: &Request,
) -> Result<Option<InputReply>> {
    let Some((error, request_id)) = reply.input_error() else {
        return Ok(None);
    };
    let Some(command) = input_context::command(request) else {
        return Ok(None);
    };
    let field =
        input_context::field(request, error).ok_or_else(runtime::client::envelope::invalid)?;
    let examples =
        super::catalog::examples(command).ok_or_else(runtime::client::envelope::invalid)?;
    let expected = if command == "batch" {
        format!("{}: {}", error.field(), error.expected())
    } else {
        error.expected().to_owned()
    };
    let error = hive_input::InputError::new(
        error.code(),
        error.message(),
        field,
        expected,
        examples.as_slice().to_vec(),
    )
    .map_err(|cause| runtime::client::envelope::invalid().with_source(cause))?;
    Ok(Some(InputReply {
        error,
        request_id: request_id.map(str::to_owned),
        path: None,
    }))
}
