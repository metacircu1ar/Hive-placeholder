//! Validate caller fields only after replay and authorization have been resolved.
use crate::{
    error::Result,
    runtime::api::{Input, InputContext},
};
use hive_input::Code;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Admin {
    #[serde(default)]
    pub action: Input<String>,
    #[serde(default)]
    pub destination: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub item: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub comb_id: Input<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminAction {
    Discard,
    ReleaseUncertain,
    Rebind,
}
impl AdminAction {
    pub(crate) fn context(self) -> InputContext {
        match self {
            Self::Discard => InputContext::CollectorDiscard,
            Self::ReleaseUncertain => InputContext::CollectorRelease,
            Self::Rebind => InputContext::CollectorRebind,
        }
    }
}
impl Admin {
    pub(crate) fn action(&self) -> Result<AdminAction> {
        let context = InputContext::CollectorDiscard;
        let expected = "discard, release_uncertain or rebind";
        let value = self.action.optional(context, "/action", expected)?;
        match value.map(String::as_str) {
            Some("discard") => Ok(AdminAction::Discard),
            Some("release_uncertain") => Ok(AdminAction::ReleaseUncertain),
            Some("rebind") => Ok(AdminAction::Rebind),
            Some(_) => Err(context.error(Code::InvalidValue, "/action", expected)),
            None => Err(context.error(Code::MissingArgument, "/action", expected)),
        }
    }

    pub(crate) fn destination(&self, action: AdminAction) -> Result<&str> {
        let context = action.context();
        for (unused, field) in [
            (
                action != AdminAction::ReleaseUncertain && !self.item.is_missing(),
                "/item",
            ),
            (
                action != AdminAction::Rebind && !self.comb_id.is_missing(),
                "/comb_id",
            ),
        ] {
            if unused {
                return Err(context.error(
                    Code::ConflictingArguments,
                    field,
                    "omit this field for the selected action",
                ));
            }
        }
        let value = self.destination.required(context, "/destination")?;
        hive_input::validate::config_name(value)
            .map_err(|rule| context.error(rule.code(), "/destination", rule.expected()))?;
        Ok(value)
    }

    pub(crate) fn comb_id(&self) -> Result<uuid::Uuid> {
        let context = InputContext::CollectorRebind;
        let value = self.comb_id.required(context, "/comb_id")?;
        hive_input::validate::uuid(value).map_err(|rule| {
            let error = context.error(rule.code(), "/comb_id", rule.expected());
            match uuid::Uuid::parse_str(value) {
                Err(cause) => error.with_source(cause),
                Ok(_) => error,
            }
        })
    }
}
