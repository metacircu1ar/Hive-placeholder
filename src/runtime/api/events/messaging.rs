//! Accepted input and approval event payloads, including omitted versus null fields.
use crate::{
    approvals::{ApprovalAction, ApprovalState},
    runtime::{
        api::{RedirectTarget, SteeringTarget},
        domain::{DeliveryMode, MessageKind},
        hub::Caller,
    },
};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct MessageAccepted<'a> {
    #[serde(skip_serializing_if = "<[crate::runtime::attachments::Attachment]>::is_empty")]
    pub attachments: &'a [crate::runtime::attachments::Attachment],
    pub id: &'a str,
    pub sender: &'a Caller,
    pub body: &'a str,
    pub sender_turn_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<DeliveryMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_target: Option<&'a SteeringTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_target: Option<&'a Option<RedirectTarget>>,
}
#[derive(Serialize)]
pub(crate) struct ApprovalRequested<'a> {
    pub approval_id: &'a str,
    pub title: Option<&'a str>,
    pub destructive: bool,
    pub state: ApprovalState,
}
#[derive(Serialize)]
pub(crate) struct ApprovalClosed<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<ApprovalOrigin<'a>>,
    pub approval_id: &'a str,
    pub state: ApprovalState,
    pub action: Option<ApprovalAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_message: Option<&'a str>,
}
#[derive(Serialize)]
pub(crate) struct ApprovalOrigin<'a> {
    pub execution: &'a str,
    pub turn: &'a str,
}
#[derive(Serialize)]
pub(crate) struct ApprovalMessage<'a> {
    pub id: &'a str,
    pub sender: &'a Caller,
    pub body: &'a str,
    pub sender_turn_id: Option<&'a str>,
    pub kind: MessageKind,
    pub approval_id: &'a str,
}
#[derive(Serialize)]
pub(crate) struct MessagePromoted<'a> {
    pub id: &'a str,
    pub not_before: i64,
    pub seq: i64,
}
#[derive(Serialize)]
pub(crate) struct MessageScheduleFailed<'a> {
    pub id: &'a str,
    pub reason: &'static str,
}

#[derive(Serialize)]
pub(crate) struct JoinEvent<'a> {
    pub join_id: &'a str,
    pub request_id: &'a str,
    pub members: usize,
    pub state: crate::runtime::api::joins::JoinState,
}
