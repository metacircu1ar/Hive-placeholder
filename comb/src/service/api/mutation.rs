use super::actions::MutationAction;
use crate::{
    Comb, Result,
    error::value,
    page::Decision,
    service::auth::Authority,
    store::{Consolidate, ReviewedWrite, Write},
    values::{ContentHash, Slug},
};
use serde_json::Value;

enum PreparedAction<'a> {
    Put(&'a Slug, &'a str, Option<&'a ContentHash>, Write<'a>),
    Append(
        &'a Slug,
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a ContentHash>,
        Write<'a>,
    ),
    Capture(&'a str, Write<'a>),
    Promote(&'a Slug, &'a ContentHash, ReviewedWrite<'a>),
    Consolidate(&'a Consolidate, Write<'a>),
    Decide(
        &'a Slug,
        &'a ContentHash,
        Decision,
        &'a str,
        ReviewedWrite<'a>,
    ),
    Remove(&'a Slug, &'a ContentHash, Write<'a>),
}

pub(crate) struct PreparedMutation<'a>(PreparedAction<'a>);

pub(crate) trait MutationPreparation<'op> {
    fn prepare<'a>(self, authority: &'a Authority) -> Result<PreparedMutation<'a>>
    where
        'op: 'a;
}
impl<'op> MutationPreparation<'op> for MutationAction<'op> {
    fn prepare<'a>(self, authority: &'a Authority) -> Result<PreparedMutation<'a>>
    where
        'op: 'a,
    {
        let required = self.required();
        let meta = self.metadata();
        let write =
            || -> Result<_> { Ok(authority.permit(required)?.write(meta.request_id, meta.run)) };
        Ok(PreparedMutation(match self {
            Self::Promote(slug, expected, _) => PreparedAction::Promote(
                slug,
                expected,
                authority.reviewed_write(meta.request_id, meta.run)?,
            ),
            Self::Decide(slug, expected, decision, reason, _) => PreparedAction::Decide(
                slug,
                expected,
                decision,
                reason,
                authority.reviewed_write(meta.request_id, meta.run)?,
            ),
            Self::Put(slug, markdown, expected, _) => {
                PreparedAction::Put(slug, markdown, expected, write()?)
            }
            Self::Append(slug, text, source, summary, expected, _) => {
                PreparedAction::Append(slug, text, source, summary, expected, write()?)
            }
            Self::Capture(text, _) => PreparedAction::Capture(text, write()?),
            Self::Consolidate(request, _) => PreparedAction::Consolidate(request, write()?),
            Self::Remove(slug, expected, _) => PreparedAction::Remove(slug, expected, write()?),
        }))
    }
}

impl PreparedMutation<'_> {
    fn target(&self) -> Option<&Slug> {
        match &self.0 {
            PreparedAction::Put(slug, ..)
            | PreparedAction::Append(slug, ..)
            | PreparedAction::Promote(slug, ..)
            | PreparedAction::Decide(slug, ..)
            | PreparedAction::Remove(slug, ..) => Some(slug),
            PreparedAction::Capture(..) | PreparedAction::Consolidate(..) => None,
        }
    }
    pub(crate) fn execute(self, comb: &mut Comb) -> Result<Value> {
        // Receipt replay precedes all access to a possibly recreated removal target.
        if let PreparedAction::Remove(slug, expected, write) = &self.0
            && let Some(removed) = comb.replay_remove(slug, expected, write)?
        {
            return value(removed);
        }
        if let Some(slug) = self.target() {
            comb.reconcile(slug)?;
        }
        match self.0 {
            PreparedAction::Put(slug, markdown, expected, write) => {
                value(comb.put_markdown(slug, markdown, expected, &write)?)
            }
            PreparedAction::Append(slug, text, source, summary, expected, write) => {
                value(comb.append(slug, text, source, summary, expected, &write)?)
            }
            PreparedAction::Capture(text, write) => value(comb.capture(text, &write)?),
            PreparedAction::Promote(slug, expected, write) => {
                value(comb.promote(slug, expected, &write)?)
            }
            PreparedAction::Consolidate(request, write) => {
                value(comb.consolidate(request, &write)?)
            }
            PreparedAction::Decide(slug, expected, decision, reason, write) => {
                value(comb.decide(slug, expected, decision, reason, &write)?)
            }
            PreparedAction::Remove(slug, expected, write) => {
                value(comb.remove(slug, expected, &write)?)
            }
        }
    }
}
