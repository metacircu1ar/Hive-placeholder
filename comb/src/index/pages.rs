//! Existing page replacement, hydration and listing queries.
use super::{Index, limits::DEFAULT_LIST_LIMIT};
use crate::store::{Document, List, Stats, Summary};
use crate::values::{ContentHash, Prefix, Slug};
use crate::{Error, Page, Result, chunk};
use rusqlite::{Connection, OptionalExtension, params};

const INSERT_PAGE: &str = "INSERT INTO pages VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)";
const INSERT_TAG: &str = "INSERT OR IGNORE INTO tags VALUES(?1,?2)";
const INSERT_ALIAS: &str = "INSERT OR IGNORE INTO aliases VALUES(?1,?2)";
const INSERT_LINK: &str = "INSERT INTO links VALUES(?1,?2,?3)";
const INSERT_IDENTITY: &str = "INSERT INTO identity_fts(slug,title,aliases) VALUES(?1,?2,?3)";
const DELETE_CHUNK_FTS: &str =
    "DELETE FROM chunk_fts WHERE rowid IN (SELECT id FROM chunks WHERE slug=?1)";
const DELETE_IDENTITY: &str = "DELETE FROM identity_fts WHERE slug=?1";
const DELETE_PAGE: &str = "DELETE FROM pages WHERE slug=?1";
const CONTENT_HASH: &str = "SELECT content_hash FROM pages WHERE slug=?1";
const PAGE_SLUGS: &str = "SELECT slug FROM pages UNION SELECT slug FROM quarantined_pages";
const LIST: &str = "SELECT slug,title,kind,content_hash,verified FROM pages p WHERE (?1 IS \
    NULL OR kind=?1) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM tags t WHERE \
    t.slug=p.slug AND t.tag=?2)) AND (?3 IS NULL OR \
    substr(slug,1,length(?3))=?3) AND (?4=0 OR verified=0) AND (?5 IS NULL OR \
    slug>?5) ORDER BY slug LIMIT ?6";
const STATS: &str = "SELECT (SELECT count(*) FROM pages),(SELECT count(*) FROM chunks),(SELECT \
    count(*) FROM links),(SELECT count(*) FROM pages WHERE \
    inbox_pending=1),(SELECT count(*) FROM pages WHERE verified=0),(SELECT \
    count(*) FROM pages WHERE unresolved_proposal=1),(SELECT count(*) FROM \
    quarantined_pages)";
const DOCUMENT: &str = "SELECT content_hash,page_json FROM pages WHERE slug=?1";

impl Index {
    pub(crate) fn upsert(&mut self, slug: &Slug, raw: &str, page: &Page) -> Result<()> {
        let tx = self.connection.transaction()?;
        Self::replace(&tx, slug, raw, page)?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn upsert_consolidation(
        &mut self,
        pair: &crate::disk::ConsolidationIntent,
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        for part in [&pair.target, &pair.note] {
            Self::replace(&tx, &part.slug, &part.raw, &part.page()?)?;
        }
        tx.commit()?;
        Ok(())
    }
    fn replace(db: &Connection, slug: &Slug, raw: &str, page: &Page) -> Result<()> {
        Self::remove(db, slug)?;
        let meta = &page.metadata;
        let parameters = params![
            slug,
            meta.authored.title,
            meta.authored.title.to_lowercase(),
            meta.authored.kind,
            ContentHash::digest(raw),
            page.verified(),
            meta.owned().external(),
            meta.owned()
                .updated_at()
                .map_or("", crate::values::Timestamp::as_str),
            page.compiled,
            crate::error::json_text(page)?,
            Prefix::is_inbox(slug) && page.inbox_pending()?,
            Prefix::is_review(slug) && meta.owned().decision().is_none(),
        ];
        db.prepare_cached(INSERT_PAGE)?.execute(parameters)?;
        for tag in &meta.authored.tags {
            db.prepare_cached(INSERT_TAG)?.execute(params![slug, tag])?;
        }
        for alias in &meta.authored.aliases {
            db.prepare_cached(INSERT_ALIAS)?
                .execute(params![slug, alias.to_lowercase()])?;
        }
        for chunk::Link { target, kind } in chunk::links(slug, page) {
            db.prepare_cached(INSERT_LINK)?
                .execute(params![slug, target, kind])?;
        }
        Self::insert_chunks(db, slug, page)?;
        db.prepare_cached(INSERT_IDENTITY)?.execute(params![
            slug,
            meta.authored.title.to_lowercase(),
            meta.authored.aliases.join(" ").to_lowercase()
        ])?;
        Ok(())
    }
    fn remove(db: &Connection, slug: &Slug) -> Result<()> {
        db.execute("DELETE FROM quarantined_pages WHERE slug=?1", [slug])?;
        db.prepare_cached(DELETE_CHUNK_FTS)?.execute([slug])?;
        db.prepare_cached(DELETE_IDENTITY)?.execute([slug])?;
        db.prepare_cached(DELETE_PAGE)?.execute([slug])?;
        Ok(())
    }
    pub(crate) fn delete(&mut self, slug: &Slug) -> Result<()> {
        let tx = self.connection.transaction()?;
        Self::remove(&tx, slug)?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn quarantine(&mut self, slug: &Slug, code: &str) -> Result<()> {
        let tx = self.connection.transaction()?;
        Self::remove(&tx, slug)?;
        tx.execute(
            "INSERT INTO quarantined_pages(slug,code) VALUES(?1,?2)",
            params![slug, code],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn content_hash(&self, slug: &Slug) -> Result<Option<ContentHash>> {
        Ok(self
            .connection
            .query_row(CONTENT_HASH, [slug], |r| r.get(0))
            .optional()?)
    }
    pub(crate) fn remove_absent(
        &mut self,
        present: &std::collections::BTreeSet<Slug>,
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        let old: Vec<Slug> = tx
            .prepare(PAGE_SLUGS)?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for slug in old {
            if !present.contains(slug.as_str()) {
                Self::remove(&tx, &slug)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
impl Index {
    pub fn list(&self, request: &List) -> Result<Vec<Summary>> {
        request.validate()?;
        let limit = request.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        let limit =
            i64::try_from(limit).map_err(|cause| Error::list_limit_overflow().caused_by(cause))?;
        Ok(self
            .connection
            .prepare(LIST)?
            .query_map(
                params![
                    request.kind,
                    request.tag,
                    request.prefix,
                    request.unverified,
                    request.after,
                    limit
                ],
                |r| {
                    Ok(Summary {
                        slug: r.get(0)?,
                        title: r.get(1)?,
                        kind: r.get(2)?,
                        hash: r.get(3)?,
                        unverified: !r.get::<_, bool>(4)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn stats(&self) -> Result<Stats> {
        Ok(self.connection.query_row(STATS, [], |r| {
            Ok(Stats {
                embedding_runtime: None,
                pages: count(r, 0)?,
                chunks: count(r, 1)?,
                links: count(r, 2)?,
                inbox: count(r, 3)?,
                unverified: count(r, 4)?,
                unresolved_proposals: count(r, 5)?,
                quarantined_pages: count(r, 6)?,
            })
        })?)
    }
}

fn count(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(column)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(e),
        )
    })
}

impl Index {
    pub fn document(&self, slug: &Slug) -> Result<Document> {
        let (hash, json): (ContentHash, String) = self
            .connection
            .query_row(DOCUMENT, [slug], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(Error::not_found)?;
        let page: crate::Page = serde_json::from_str(&json)?;
        Ok(Document {
            slug: slug.into(),
            hash,
            markdown: page.render()?,
            unverified: !page.verified(),
            page,
        })
    }
}
