//! Translate only known server-side write coordinates into the submitted tool.
use crate::{Error, service::input::mutation::Write};

pub fn tool_error(name: &str, error: Error) -> Error {
    let Some(input) = error.input() else {
        return error;
    };
    let Some(kind) = name.strip_prefix("comb_").and_then(Write::named) else {
        return error;
    };
    let field = std::iter::once("")
        .chain(kind.fields().iter().copied())
        .find(|field| kind.pointer(field, false) == input.field());
    let mut mapped = match field {
        Some(field) => match hive_input::InputError::new(
            input.code(),
            input.message(),
            kind.pointer(field, true),
            input.expected(),
            vec![kind.example(true)],
        ) {
            Ok(input) => Error::from(input),
            Err(cause) => Error::internal_serialization().caused_by(cause),
        },
        None => Error::invalid_input_reply(),
    };
    mapped.file_committed = error.file_committed;
    mapped.request_id.clone_from(&error.request_id);
    mapped.import_progress.clone_from(&error.import_progress);
    mapped.caused_by(error)
}

#[cfg(test)]
mod tests;
