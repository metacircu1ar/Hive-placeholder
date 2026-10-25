//! A review grant owns the write attribution it authorized.
use super::Write;
use crate::{Error, Result, service::auth::ReviewPermission, values::RequestId};

#[derive(Debug)]
pub struct ReviewedWrite<'a> {
    write: Write<'a>,
}
impl<'a> ReviewedWrite<'a> {
    pub(crate) fn authorized(
        permission: ReviewPermission<'a>,
        request_id: &RequestId,
        run: Option<&'a str>,
    ) -> Self {
        Self {
            write: permission.write(request_id, run),
        }
    }
    pub(super) fn write(&self) -> &Write<'a> {
        &self.write
    }
}
impl<'a> Write<'a> {
    pub fn review_for_promotion(self, granted: bool) -> Result<ReviewedWrite<'a>> {
        self.reviewed(granted, Error::promotion_reviewer_required)
    }
    pub fn review_for_decision(self, granted: bool) -> Result<ReviewedWrite<'a>> {
        self.reviewed(granted, Error::decision_reviewer_required)
    }
    fn reviewed(self, granted: bool, denied: fn() -> Error) -> Result<ReviewedWrite<'a>> {
        if !self.actor.is_human() && !granted {
            return Err(denied());
        }
        Ok(ReviewedWrite { write: self })
    }
}
