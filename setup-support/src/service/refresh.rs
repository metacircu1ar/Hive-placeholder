//! Journal a stopped unit refresh independently of binary publication.
use super::{Controller, Spec, held};
use crate::{Result, files, invalid};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Refresh {
    old: Spec,
    next: Spec,
}
impl Controller<'_> {
    pub(super) fn recover_refresh(&self) -> Result<()> {
        let path = self
            .layout
            .descriptor(self.scope)?
            .with_extension("refresh.json");
        if let Some(change) = files::json::<Refresh>(&path)? {
            held::restore_artifact(
                self.layout,
                self.scope,
                self.runner,
                &change.old,
                &change.next,
                true,
            )?;
            files::remove(&path, &files::encode(&change)?)?;
        }
        Ok(())
    }
    pub(super) fn refresh_unit(&self, old: &Spec, next: &Spec) -> Result<()> {
        let mut comparable = old.clone();
        comparable.unit_contents.clone_from(&next.unit_contents);
        if comparable != *next {
            return Err(invalid(
                "Owned service settings differ; uninstall before changing installation settings.",
            ));
        }
        old.verify(self.layout, self.scope)?;
        if !self.manager().available()?
            || self.manager().state(self.scope)? == super::manager::State::Loaded
        {
            return Err(invalid(
                "Owned unit needs refresh; run service stop, then service install and service \
                    start.",
            ));
        }
        let path = self
            .layout
            .descriptor(self.scope)?
            .with_extension("refresh.json");
        files::write(
            &path,
            &files::encode(&Refresh {
                old: old.clone(),
                next: next.clone(),
            })?,
            false,
        )?;
        self.recover_refresh()
    }
}
