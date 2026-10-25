//! Private selections reference the frozen baseline; public selections remain complete.
use super::{OptionsSelection, ReservedTurn};
use crate::options::{Snapshot, SnapshotContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    model: String,
    effort: String,
    revision: u64,
    #[serde(default, deserialize_with = "inline_snapshot")]
    options: Option<Snapshot>,
}

// Only omission means a baseline reference. A present null/string/array is invalid.
fn inline_snapshot<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Snapshot>, D::Error> {
    super::decode::object(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reservation {
    message_id: String,
    #[serde(deserialize_with = "super::decode::object")]
    selection: Selection,
}

impl Selection {
    fn hydrate(self, baseline: Option<&Arc<Snapshot>>) -> Result<OptionsSelection, &'static str> {
        let options = match self.options {
            None => baseline
                .ok_or("A public selection cannot reference a private baseline.")?
                .clone(),
            Some(options) => {
                if baseline.is_some_and(|baseline| baseline.as_ref() == &options) {
                    return Err("A stored baseline selection must use an explicit reference.");
                }
                Arc::new(options)
            }
        };
        Ok(OptionsSelection {
            model: self.model,
            effort: self.effort,
            revision: self.revision,
            options,
        })
    }
}

pub(super) struct Records {
    pub reserved_turn: Option<Reservation>,
    pub session_selection: Option<Selection>,
    pub last_accepted_selection: Option<Selection>,
    pub turn_selection: Option<Selection>,
}
pub(super) struct Hydrated {
    pub reserved_turn: Option<ReservedTurn>,
    pub session_selection: Option<OptionsSelection>,
    pub last_accepted_selection: Option<OptionsSelection>,
    pub turn_selection: Option<OptionsSelection>,
}
impl Records {
    pub(super) fn hydrate(self, context: &impl SnapshotContext) -> Result<Hydrated, &'static str> {
        let baseline = context.baseline_snapshot();
        Ok(Hydrated {
            reserved_turn: self
                .reserved_turn
                .map(|reserved| -> Result<_, &'static str> {
                    Ok(ReservedTurn {
                        message_id: reserved.message_id,
                        selection: reserved.selection.hydrate(baseline)?,
                    })
                })
                .transpose()?,
            session_selection: self
                .session_selection
                .map(|s| s.hydrate(baseline))
                .transpose()?,
            last_accepted_selection: self
                .last_accepted_selection
                .map(|s| s.hydrate(baseline))
                .transpose()?,
            turn_selection: self
                .turn_selection
                .map(|s| s.hydrate(baseline))
                .transpose()?,
        })
    }
}

#[derive(Serialize)]
pub(super) struct SelectionRef<'a> {
    model: &'a str,
    effort: &'a str,
    revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<&'a Snapshot>,
}
impl<'a> SelectionRef<'a> {
    fn new(value: &'a OptionsSelection, baseline: Option<&Arc<Snapshot>>) -> Self {
        Self {
            model: &value.model,
            effort: &value.effort,
            revision: value.revision,
            options: if baseline == Some(&value.options) {
                None
            } else {
                Some(value.options.as_ref())
            },
        }
    }
}

#[derive(Serialize)]
pub(super) struct ReservationRef<'a> {
    message_id: &'a str,
    selection: SelectionRef<'a>,
}
pub(super) fn reservation<'a>(
    value: Option<&'a ReservedTurn>,
    baseline: Option<&Arc<Snapshot>>,
) -> Option<ReservationRef<'a>> {
    value.map(|reserved| ReservationRef {
        message_id: &reserved.message_id,
        selection: SelectionRef::new(&reserved.selection, baseline),
    })
}
pub(super) fn selection<'a>(
    value: Option<&'a OptionsSelection>,
    baseline: Option<&Arc<Snapshot>>,
) -> Option<SelectionRef<'a>> {
    value.map(|value| SelectionRef::new(value, baseline))
}
