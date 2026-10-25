//! Caller-input classification is local and never inferred from runtime codes.
use super::hints::{Exclusive, Hint};
use comb::Error;
use hive_input::{InputError, Product, Rendered};
use std::path::{Path, PathBuf};

#[derive(Debug)]
enum Kind {
    Input(Box<InputError>),
    Operational(Box<Error>),
}
pub(crate) struct Failure {
    kind: Kind,
    path: Option<PathBuf>,
    cause: Option<Box<Error>>,
    hint: Option<Hint>,
}
impl std::fmt::Debug for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.kind.fmt(f)
    }
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self {
            kind: Kind::Operational(Box::new(error)),
            path: None,
            cause: None,
            hint: None,
        }
    }
}
impl Failure {
    pub(in crate::cli) fn remote(
        error: Error,
        command: &'static str,
        source: Option<&'static str>,
    ) -> Self {
        let Some(input) = error.input() else {
            let hint = Hint::query(&error, command);
            return Self {
                hint,
                ..Self::from(error)
            };
        };
        let Some(field) = super::remote::field(command, input.field(), source) else {
            let mut invalid = Error::invalid_input_reply();
            invalid.file_committed = error.file_committed;
            invalid.request_id = error.request_id;
            invalid.import_progress = error.import_progress;
            return Self::from(invalid);
        };
        let Some(examples) = crate::cli::catalog::examples(command) else {
            return Self::from(error);
        };
        let mapped = InputError::new(
            input.code(),
            input.message(),
            field,
            input.expected(),
            examples.as_slice().to_vec(),
        );
        match mapped {
            Ok(input) => {
                let mut mapped = Error::from(input);
                mapped.file_committed = error.file_committed;
                mapped.request_id.clone_from(&error.request_id);
                mapped.import_progress.clone_from(&error.import_progress);
                Self::from(mapped.caused_by(error))
            }
            Err(cause) => Self::from(Error::internal_serialization().caused_by(cause)),
        }
    }
    pub(in crate::cli) fn exclusive(mut self, operation: Option<Exclusive>) -> Self {
        if let Kind::Operational(error) = &self.kind
            && error.code == comb::ErrorCode::CombBusy
        {
            self.hint = operation.map(Hint::Exclusive);
        }
        self
    }
    pub(in crate::cli) fn with_cleanup(self, cleanup: comb::Result<()>) -> Self {
        let Err(cleanup) = cleanup else {
            return self;
        };
        let primary = match self.kind {
            Kind::Operational(error) => *error,
            Kind::Input(error) => Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                *error,
            )),
        };
        let primary = match self.cause {
            Some(cause) => primary.caused_by(*cause),
            None => primary,
        };
        Self {
            hint: self.hint,
            ..Self::from(comb::service::auth::with_cleanup(primary, Err(cleanup)))
                .path(self.path.as_deref())
        }
    }

    pub(in crate::cli) fn render_mcp(self) -> Rendered {
        // MCP stderr never includes a selected path, received value or native cause.
        let (stderr, exit_code) = match self.kind {
            Kind::Input(error) => match error.render(Product::Comb) {
                Ok(output) => (
                    format!("{}: {}", error.code().as_str(), output.stderr),
                    output.exit_code,
                ),
                Err(cause) => {
                    return Self::from(Error::internal_serialization().caused_by(cause))
                        .render_mcp();
                }
            },
            Kind::Operational(error) => (
                format!(
                    "{}: MCP startup or transport failed. Check credential access and Comb \
                        service status.\nExample: comb mcp --credential /private/client.json\n",
                    error.code
                ),
                1,
            ),
        };
        Rendered {
            stdout: String::new(),
            stderr,
            exit_code,
        }
    }

    pub(super) fn input(error: InputError) -> Self {
        Self {
            kind: Kind::Input(Box::new(error)),
            path: None,
            cause: None,
            hint: None,
        }
    }
    pub(super) fn caused_by(mut self, cause: Error) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }
    #[cfg(test)]
    pub(in crate::cli) fn cause(&self) -> Option<&Error> {
        self.cause.as_deref()
    }
    pub(super) fn path(mut self, path: Option<&Path>) -> Self {
        self.path = path.map(Path::to_path_buf);
        self
    }
    pub(in crate::cli) fn code(&self) -> &'static str {
        match &self.kind {
            Kind::Input(error) => error.code().as_str(),
            Kind::Operational(error) => error.code.as_str(),
        }
    }
    pub(in crate::cli) fn operational(&self) -> Option<&Error> {
        match &self.kind {
            Kind::Operational(error) => Some(error),
            Kind::Input(_) => None,
        }
    }
    pub(in crate::cli) fn render(self) -> Rendered {
        let mut output = match self.kind {
            Kind::Input(error) => error.render(Product::Comb).unwrap_or_else(|cause| {
                Self::from(Error::internal_serialization().caused_by(cause)).render()
            }),
            Kind::Operational(error) if error.input().is_some() => {
                match error
                    .input()
                    .map(|input| input.render(Product::Comb))
                    .transpose()
                {
                    Ok(Some(mut output)) => {
                        output.stdout = format!("{}\n", crate::cli::replies::failure(error));
                        output
                    }
                    _ => Self::from(Error::internal_serialization()).render(),
                }
            }
            Kind::Operational(error) => {
                let stderr = match self.hint {
                    Some(hint) => match hint.render(&error) {
                        Ok(text) => text,
                        Err(cause) => return Self::from(cause).render(),
                    },
                    None => error
                        .diagnostic()
                        .map_or_else(String::new, |d| format!("{d}\n")),
                };
                Rendered {
                    stderr,
                    stdout: format!("{}\n", crate::cli::replies::failure(error)),
                    exit_code: 1,
                }
            }
        };
        if let Some(diagnostic) = self
            .cause
            .as_deref()
            .and_then(super::hints::socket_directory)
        {
            output.stderr.push_str(&diagnostic);
            output.stderr.push('\n');
        }
        if let Some(path) = &self.path {
            output.stderr.push_str(&path_line(path));
        }
        output
    }
}

fn path_line(path: &Path) -> String {
    let mut text = String::new();
    for byte in path.as_os_str().as_encoded_bytes() {
        let escaped = std::ascii::escape_default(*byte).to_string();
        if text.len() + escaped.len() > 200 {
            text.push_str("...");
            break;
        }
        text.push_str(&escaped);
    }
    format!("Path: {text}\n")
}
