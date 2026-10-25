//! Checked capabilities stay bound to the immutable identity that received them.
use super::Authority;
use crate::{
    Actor, Error, Result,
    service::api::Capability,
    store::{ReviewedWrite, Write},
    values::RequestId,
};

pub(crate) struct Permission<'a> {
    authority: &'a Authority,
}
pub(crate) struct ReviewPermission<'a> {
    authority: &'a Authority,
}

impl Authority {
    pub(crate) fn collector_admission(&self, required: Capability) -> Result<()> {
        if matches!(self.actor(), Actor::Collector { .. }) && required != Capability::Capture {
            return Err(Error::collector_capture_only());
        }
        Ok(())
    }
    pub(crate) fn permit(&self, required: Capability) -> Result<Permission<'_>> {
        match required {
            Capability::Human => self.human()?,
            Capability::Review if !self.reviewer => {
                return Err(Error::configured_reviewer_required());
            }
            Capability::Read | Capability::Write | Capability::Capture | Capability::Review => {}
        }
        self.collector_admission(required)?;
        Ok(Permission { authority: self })
    }
    pub(crate) fn reviewed_write<'a>(
        &'a self,
        request_id: &RequestId,
        run: Option<&'a str>,
    ) -> Result<ReviewedWrite<'a>> {
        let permission = self.permit(Capability::Review)?;
        Ok(ReviewedWrite::authorized(
            ReviewPermission {
                authority: permission.authority,
            },
            request_id,
            run,
        ))
    }
}
impl<'a> Permission<'a> {
    pub(crate) fn write(self, request_id: &RequestId, run: Option<&'a str>) -> Write<'a> {
        self.authority.write(request_id, run)
    }
}
impl<'a> ReviewPermission<'a> {
    pub(crate) fn write(self, request_id: &RequestId, run: Option<&'a str>) -> Write<'a> {
        self.authority.write(request_id, run)
    }
}

#[cfg(test)]
mod tests;
