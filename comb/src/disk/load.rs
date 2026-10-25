//! Read a captured page and authenticate its ownership without publishing it.
use super::{ContentHash, Disk, Page, Result, Slug};

pub(crate) struct Loaded {
    pub raw: String,
    pub hash: ContentHash,
    pub page: Page,
    pub trusted: bool,
}

impl Disk {
    pub(crate) fn load(&self, slug: &Slug) -> Result<Option<Loaded>> {
        self.content(slug)?
            .map(|raw| {
                let page = Page::parse(&raw)?;
                self.load_parsed(slug, raw, page)
            })
            .transpose()
    }

    // Scan callers retain their original raw bytes and parse-error policy.
    pub(crate) fn load_parsed(&self, slug: &Slug, raw: String, mut page: Page) -> Result<Loaded> {
        let trusted = self.trusted(slug, &page, &raw)?;
        if !trusted {
            page.externalize();
        }
        Ok(Loaded {
            hash: ContentHash::digest(&raw),
            raw,
            page,
            trusted,
        })
    }
}
