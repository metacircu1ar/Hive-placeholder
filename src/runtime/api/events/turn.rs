//! Input-to-turn bindings are typed before any delivery or call-result SQL runs.
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct TurnStarted<'a> {
    #[serde(skip_serializing_if = "<[crate::runtime::attachments::Attachment]>::is_empty")]
    pub attachments: &'a [crate::runtime::attachments::Attachment],
    pub turn_id: &'a str,
    pub message_id: &'a str,
    pub sender: &'a str,
    #[serde(flatten)]
    pub selection: TurnSelection<'a>,
}
#[derive(Serialize)]
pub(crate) struct TurnSelection<'a> {
    pub model: &'a str,
    pub effort: &'a str,
    pub options_revision: u64,
    pub selection_verification: &'a str,
}
