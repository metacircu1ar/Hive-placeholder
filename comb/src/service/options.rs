//! Model-dependent startup options preserve omission versus an explicit default.
use super::{Error, Options, Result};
use hive_input::{Code, InputError};

impl Options {
    pub(crate) fn model_option(&self) -> Option<&'static str> {
        if self.replace_embeddings {
            Some("replace_embeddings")
        } else if self.retry_failed {
            Some("retry_failed")
        } else if self.vector_cache_mib.is_some() {
            Some("vector_cache_mib")
        } else {
            None
        }
    }
    pub(crate) fn validate_model(&self, selected: bool) -> Result<()> {
        let Some(field) = self.model_option().filter(|_| !selected) else {
            return Ok(());
        };
        let error = InputError::new(
            Code::MissingArgument,
            "This serve option requires an embedding model.",
            format!("/options/{field}"),
            "an explicit --embedding-config or a model selected through setup; \
                otherwise omit embedding-only flags",
            vec![
                "comb --root /private/knowledge serve --embedding-config /private/embeddings.yaml"
                    .into(),
            ],
        )
        .map_err(|cause| Error::internal_serialization().caused_by(cause))?;
        Err(error.into())
    }
    pub(crate) fn recheck_model(&self, selected: bool) -> Result<()> {
        self.validate_model(selected)
            .map_err(|cause| Error::serve_selection_changed().caused_by(cause))
    }
}
