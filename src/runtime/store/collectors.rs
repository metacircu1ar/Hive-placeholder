//! Evidence retention independent of agent/event retention.
pub(super) mod capture;
mod delivery;
mod queries;
use super::{Queries, Store};
use crate::{
    config::{CollectionSource, CollectorConfig},
    error::{Error, Result},
    runtime::collectors::{Destination, Status, UncertainItem},
};
use rusqlite::params;
use std::collections::BTreeMap;

impl Store {
    pub(in crate::runtime) fn configure_collectors(
        &self,
        config: &BTreeMap<String, CollectorConfig>,
        identities: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.atomic(|_| {
            self.execute_cached(queries::CONFIGURE_COLLECTORS_UPDATE, [])?;
            for (name, settings) in config {
                self.execute_cached(
                    queries::CONFIGURE_COLLECTORS_INSERT,
                    params![
                        name,
                        settings
                            .credential_file
                            .to_str()
                            .ok_or_else(|| Error::config_invalid(
                                "Collector paths must be UTF-8."
                            ))?,
                        serde_json::to_string(&settings.sources).map_err(|e| {
                            Error::internal_error("Cannot encode collector sources.").with_source(e)
                        })?,
                        settings.enabled,
                        identities.get(name)
                    ],
                )?;
            }
            let count: i64 =
                self.sql()
                    .cached()
                    .query_row(queries::CONFIGURE_COLLECTORS_SELECT, [], |r| r.get(0))?;
            if count > 16 {
                return Err(Error::config_invalid(
                    "At most 16 retained Comb destinations are allowed; reuse existing names.",
                ));
            }
            if config.values().any(|c| c.enabled) {
                self.collectors_possible.set(true);
            }
            Ok(())
        })
    }
}
impl Queries {
    pub(in crate::runtime) fn collector_destinations(&self) -> Result<Vec<Destination>> {
        self.sql().cached().query_map(
            queries::COLLECTOR_DESTINATIONS_SELECT,
            [super::convert::now()?],
            |r| {
                Ok(Destination {
                    name: r.get(0)?,
                    credential_file: r.get::<_, String>(1)?.into(),
                    revision: r.get(2)?,
                })
            },
        )
    }
    pub(in crate::runtime) fn collector_status(&self) -> Result<Vec<Status>> {
        let mut rows =
            self.sql()
                .cached()
                .query_map(queries::COLLECTOR_STATUS_SELECT, [], |r| {
                    Ok(Status {
                        name: r.get(0)?,
                        enabled: r.get(1)?,
                        paused: r.get(2)?,
                        comb_id: r.get(3)?,
                        credential_id: r.get(4)?,
                        pending: r.get(5)?,
                        bytes: r.get(6)?,
                        uncertain: r.get(7)?,
                        gap_count: r.get(8)?,
                        gap_first: r.get(9)?,
                        gap_last: r.get(10)?,
                        last_error: r.get(11)?,
                        uncertain_items: Vec::new(),
                    })
                })?;
        for row in &mut rows {
            row.uncertain_items = self.sql().cached().query_map(
                queries::COLLECTOR_STATUS_SELECT_2,
                [&row.name],
                |r| {
                    Ok(UncertainItem {
                        id: r.get(0)?,
                        generation: r.get(1)?,
                        comb_id: r.get(2)?,
                        credential_id: r.get(3)?,
                        last_error: r.get(4)?,
                    })
                },
            )?;
        }
        Ok(rows)
    }
}
fn sources(value: &str) -> Result<Vec<CollectionSource>> {
    serde_json::from_str(value)
        .map_err(|e| Error::database_error("Invalid saved collector sources.").with_source(e))
}
fn key(parts: impl serde::Serialize) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&parts)
        .map_err(|e| Error::internal_error("Cannot encode collector identity.").with_source(e))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
