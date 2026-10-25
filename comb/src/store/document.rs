use super::{Comb, Document, Error, Result, Slug};
use crate::disk::Loaded;

impl Comb {
    pub fn get(&self, slug: &Slug) -> Result<Document> {
        self.ready()?;
        let loaded = self.disk.load(slug)?.ok_or_else(Error::not_found)?;
        Document::from_loaded(slug, loaded)
    }
}

impl Document {
    fn from_loaded(slug: &Slug, loaded: Loaded) -> Result<Self> {
        let unverified = !loaded.page.verified();
        // Keep rendering at the old get boundary, before mutation CAS or replay.
        // The raw page can fit the limit while its sanitized rendering exceeds it.
        Ok(Self {
            slug: slug.into(),
            hash: loaded.hash,
            markdown: loaded.page.render()?,
            page: loaded.page,
            unverified,
        })
    }
}
