mod queries;
use super::{Destination, Result, Store, params};
use crate::runtime::collectors::{Admin, AdminAction, Delivery};
use hive_input::Code;

impl Store {
    /// Called only after authenticating the exact connection outside the writer.
    pub(in crate::runtime) fn claim_collection(
        &self,
        destination: &Destination,
        comb: &str,
        credential: &str,
    ) -> Result<Option<Delivery>> {
        self.atomic(|store| {
            let current: Option<(Option<String>, i64, bool)> = store.sql().cached().query_opt(
                queries::CLAIM_COLLECTION_SELECT,
                [&destination.name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let Some((saved, revision, paused)) = current else {
                return Ok(None);
            };
            if revision != destination.revision || paused {
                return Ok(None);
            }
            if saved.as_deref().is_some_and(|old| old != comb) {
                store.execute_cached(queries::CLAIM_COLLECTION_UPDATE, [&destination.name])?;
                return Ok(None);
            }
            store.execute_cached(
                queries::CLAIM_COLLECTION_UPDATE_2,
                params![destination.name, comb, credential],
            )?;
            store.collector_gap(&destination.name)?;
            let selected = store.sql().cached().query_opt(
                queries::CLAIM_COLLECTION_SELECT_2,
                params![
                    destination.name,
                    super::super::convert::now()?,
                    comb,
                    credential
                ],
                |r| {
                    Ok(Delivery {
                        id: r.get(0)?,
                        destination: destination.name.clone(),
                        generation: r.get(1)?,
                        body: r.get(2)?,
                        revision,
                    })
                },
            )?;
            if let Some(item) = &selected {
                store.execute_cached(
                    queries::CLAIM_COLLECTION_UPDATE_3,
                    params![item.id, comb, credential],
                )?;
            } else {
                store.execute_cached(
                    queries::CLAIM_COLLECTION_UPDATE_4,
                    params![
                        destination.name,
                        super::super::convert::now()?.saturating_add(5),
                        comb,
                        credential
                    ],
                )?;
            }
            Ok(selected)
        })
    }
    pub(in crate::runtime) fn collection_destination_error(
        &self,
        destination: &Destination,
        code: &str,
    ) -> Result<()> {
        self.execute_cached(
            queries::COLLECTION_DESTINATION_ERROR_UPDATE,
            params![
                destination.name,
                destination.revision,
                code,
                super::super::convert::now()?.saturating_add(5)
            ],
        )?;
        Ok(())
    }
    pub(in crate::runtime) fn finish_collection(
        &self,
        item: &Delivery,
        error: Option<&str>,
    ) -> Result<()> {
        self.atomic(|store| {
            let updated = if let Some(code) = error {
                store.execute_cached(
                    queries::FINISH_COLLECTION_UPDATE,
                    params![
                        item.id,
                        item.generation,
                        code,
                        super::super::convert::now()?.saturating_add(5),
                        item.destination,
                        item.revision
                    ],
                )?
            } else {
                store.execute_cached(
                    queries::FINISH_COLLECTION_DELETE,
                    params![item.id, item.generation, item.destination, item.revision],
                )?
            };
            if updated != 0 {
                store.execute_cached(
                    queries::FINISH_COLLECTION_UPDATE_2,
                    params![item.destination, error],
                )?;
                store.collector_gap(&item.destination)?;
            }
            Ok(())
        })
    }
    pub(in crate::runtime) fn collector_admin(&self, command: &Admin) -> Result<()> {
        self.atomic(|store| {
            let action = command.action()?;
            let context = action.context();
            let destination = command.destination(action)?;
            let exists: bool = store.sql().cached().query_row(
                queries::COLLECTOR_ADMIN_SELECT,
                [destination],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(context.error(
                    Code::InvalidValue,
                    "/destination",
                    "a retained collector destination; inspect collectors status",
                ));
            }
            match action {
                AdminAction::Discard => {
                    store.execute_cached(queries::COLLECTOR_ADMIN_DELETE, [destination])?;
                    store.execute_cached(queries::COLLECTOR_ADMIN_UPDATE, [destination])?;
                }
                AdminAction::ReleaseUncertain => {
                    let item = command.item.required(context, "/item")?;
                    let changed = store.execute_cached(
                        queries::COLLECTOR_ADMIN_UPDATE_2,
                        params![item, destination],
                    )?;
                    if changed == 0 {
                        return Err(context.error(
                            Code::InvalidValue,
                            "/item",
                            "an attempted item at this destination; inspect collectors status",
                        ));
                    }
                    store.execute_cached(queries::COLLECTOR_ADMIN_UPDATE_3, [destination])?;
                }
                AdminAction::Rebind => {
                    let comb_id = command.comb_id()?.to_string();
                    store.execute_cached(
                        queries::COLLECTOR_ADMIN_UPDATE_4,
                        params![destination, comb_id],
                    )?;
                }
            }
            Ok(())
        })
    }
}
