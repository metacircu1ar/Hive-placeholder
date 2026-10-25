//! Explicit test fixtures that intentionally bypass durable page mutation.
use super::Index;
use crate::values::{ContentHash, Fingerprint};
use rusqlite::params;

impl Index {
    pub(crate) fn reject_page_insert(&self) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "CREATE TRIGGER reject_page BEFORE INSERT ON pages BEGIN \
            SELECT RAISE(FAIL,'injected'); END;",
        )
    }
    pub(crate) fn allow_page_insert(&self) -> rusqlite::Result<()> {
        self.connection.execute_batch("DROP TRIGGER reject_page")
    }
    pub(crate) fn reject_page_delete(&self) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "CREATE TRIGGER reject_remove BEFORE DELETE ON pages \
            BEGIN SELECT RAISE(FAIL,'injected removal failure'); END;",
        )
    }
    pub(crate) fn allow_page_delete(&self) -> rusqlite::Result<()> {
        self.connection.execute_batch("DROP TRIGGER reject_remove")
    }
    pub(crate) fn add_unexpected_table(&self) -> rusqlite::Result<()> {
        self.connection
            .execute_batch("CREATE TABLE unexpected(x TEXT)")
    }
    pub(crate) fn seed_graph_with_back_edges(&mut self) -> rusqlite::Result<()> {
        let tx = self.connection.transaction()?;
        for i in 0..999 {
            let slug = format!("concepts/node-{i}");
            tx.execute(
                "INSERT INTO pages SELECT ?1,title,title_key,kind,content_hash,verified,\
                external,updated_at,compiled,page_json,inbox_pending,unresolved_proposal FROM \
                pages WHERE slug='concepts/root'",
                [&slug],
            )?;
            tx.execute(
                "INSERT INTO links VALUES('concepts/root',?1,'slug'),(?1,'concepts/root','slug')",
                [&slug],
            )?;
        }
        tx.commit()
    }
    pub(crate) fn add_graph_overflow_neighbor(&self) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO links VALUES('concepts/root','concepts/extra','slug')",
            [],
        )?;
        Ok(())
    }
    pub(crate) fn seed_missing_link_beyond_page(&mut self) -> rusqlite::Result<()> {
        let tx = self.connection.transaction()?;
        for n in 0..1000 {
            let slug = format!("concepts/target-{n:04}");
            tx.execute(
                "INSERT INTO pages SELECT ?1,title,title_key,kind,content_hash,verified,\
                external,updated_at,compiled,page_json,inbox_pending,unresolved_proposal FROM \
                pages WHERE slug='concepts/source'",
                [&slug],
            )?;
            tx.execute(
                "INSERT INTO links VALUES('concepts/source',?1,'slug')",
                [&slug],
            )?;
        }
        tx.execute(
            "INSERT INTO links VALUES('concepts/source','concepts/zz-missing','slug')",
            [],
        )?;
        tx.commit()
    }
    pub(crate) fn seed_finding_limit_links(&mut self) -> rusqlite::Result<()> {
        let tx = self.connection.transaction()?;
        for n in 0..1024 {
            tx.execute(
                "INSERT INTO links VALUES('concepts/source',?1,'slug')",
                [format!("concepts/missing-{n:04}")],
            )?;
        }
        tx.commit()
    }
    pub(crate) fn seed_invalid_graph_neighbors(&self) -> rusqlite::Result<()> {
        self.connection.execute_batch("BEGIN")?;
        for position in 0..1000 {
            // Invalid Slug sorting first proves whether traversal reached decoding.
            let target = if position == 0 {
                "!invalid".into()
            } else {
                format!("concepts/neighbor-{position:04}")
            };
            self.connection.execute(
                "INSERT INTO links VALUES('concepts/root',?1,'slug')",
                [target],
            )?;
        }
        self.connection.execute_batch("COMMIT")
    }
    pub(crate) fn add_graph_degree_sentinel(&self) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO links VALUES('concepts/root','concepts/sentinel','slug')",
            [],
        )?;
        Ok(())
    }
    pub(crate) fn seed_filter_vectors(
        &self,
        fingerprint: &Fingerprint,
        bytes: &[u8],
    ) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO vector_cache.vectors SELECT DISTINCT ?1,input_hash,?2 FROM chunks",
            params![fingerprint, bytes],
        )?;
        Ok(())
    }
    pub(crate) fn corrupt_cached_vectors(&self) -> rusqlite::Result<()> {
        self.connection
            .execute("UPDATE vector_cache.vectors SET vector=zeroblob(8)", [])?;
        Ok(())
    }
    pub(crate) fn seed_embedding_failures(
        &mut self,
        fingerprint: &Fingerprint,
    ) -> rusqlite::Result<()> {
        let tx = self.connection.transaction()?;
        // Populate only derived state to avoid 1025 unrelated durable writes.
        tx.execute("DELETE FROM chunks", [])?;
        for ordinal in 0..1025 {
            tx.execute(
                "INSERT INTO chunks \
                VALUES(?1,'concepts/failures',?1,'compiled','','',1,'hash','instance')",
                [ordinal],
            )?;
            tx.execute(
                "INSERT INTO embedding_failures VALUES(?1,?2,'rejected')",
                params![ordinal, fingerprint],
            )?;
        }
        tx.commit()
    }
    pub(crate) fn insert_malformed_vector(
        &self,
        fingerprint: &Fingerprint,
        hash: &ContentHash,
    ) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO vector_cache.vectors VALUES(?1,?2,zeroblob(8))",
            params![fingerprint, hash],
        )?;
        Ok(())
    }
    pub(crate) fn reject_second_vector(
        &self,
        fingerprint: &Fingerprint,
        hash: &ContentHash,
    ) -> rusqlite::Result<()> {
        self.connection.execute_batch(&format!(
            "CREATE TEMP TRIGGER reject_second_vector \
             BEFORE INSERT ON vector_cache.vectors \
             WHEN NEW.fingerprint='{}' AND NEW.input_hash='{}' \
             BEGIN SELECT RAISE(ABORT,'second vector fixture'); END",
            fingerprint.as_str(),
            hash.as_str(),
        ))
    }
    pub(crate) fn allow_second_vector(&self) -> rusqlite::Result<()> {
        self.connection
            .execute_batch("DROP TRIGGER temp.reject_second_vector")
    }
}
