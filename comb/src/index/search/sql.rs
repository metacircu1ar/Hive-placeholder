//! Compile-time query assembly keeps every retrieval route distinct.
macro_rules! filter {
    () => {
        "(:kind IS NULL OR p.kind=:kind) AND (:tag IS NULL OR EXISTS(SELECT 1 FROM \
            tags WHERE slug=p.slug AND tag=:tag)) AND (:prefix IS NULL OR \
            substr(p.slug,1,length(:prefix))=:prefix) AND (:after IS NULL OR \
            julianday(p.updated_at)>=julianday(:after)) AND (:before IS NULL OR \
            julianday(p.updated_at)<=julianday(:before))"
    };
}
macro_rules! columns {
    () => {
        "c.id,p.slug,c.zone='compiled',c.verified,instr(p.title_key,:lower)>0,\
        unixepoch(p.updated_at),(p.title_key=:lower OR p.slug=:lower)"
    };
}
#[cfg(test)]
pub(super) use columns;

pub(super) const FTS: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM chunk_fts JOIN chunks c ON c.id=chunk_fts.rowid JOIN pages p ON \
        p.slug=c.slug WHERE chunk_fts MATCH :query AND ",
    filter!(),
    " ORDER BY bm25(chunk_fts),p.slug,c.ordinal"
);
pub(super) const IDENTITY_SHORT: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM pages p JOIN chunks c ON c.slug=p.slug AND c.ordinal=0 WHERE \
        (instr(lower(p.slug),:query)>0 OR instr(p.title_key,:query)>0 OR \
        EXISTS(SELECT 1 FROM aliases WHERE slug=p.slug AND instr(alias,:query)>0)) \
        AND ",
    filter!(),
    " ORDER BY p.slug"
);
pub(super) const IDENTITY_LONG: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM pages p JOIN chunks c ON c.slug=p.slug AND c.ordinal=0 WHERE p.slug \
        IN (SELECT slug FROM identity_fts WHERE identity_fts MATCH :query) AND ",
    filter!(),
    " ORDER BY p.slug"
);
pub(super) const EXACT: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM pages p JOIN chunks c ON c.slug=p.slug AND c.ordinal=0 WHERE \
        (p.slug=:query OR p.title_key=:query OR EXISTS(SELECT 1 FROM aliases WHERE \
        slug=p.slug AND alias=:query)) AND ",
    filter!()
);
pub(super) const VECTOR_HASH: &str = concat!(
    "SELECT ",
    columns!(),
    ",v.input_hash FROM chunks c JOIN pages p ON p.slug=c.slug JOIN \
        vector_cache.vectors v ON v.input_hash=c.input_hash AND \
        v.fingerprint=:fingerprint WHERE ",
    filter!(),
    " ORDER BY c.slug,c.ordinal"
);
pub(super) const VECTOR_BLOB: &str = concat!(
    "SELECT ",
    columns!(),
    ",v.vector FROM chunks c JOIN pages p ON p.slug=c.slug JOIN \
        vector_cache.vectors v ON v.input_hash=c.input_hash AND \
        v.fingerprint=:fingerprint WHERE ",
    filter!(),
    " ORDER BY c.slug,c.ordinal"
);
pub(in crate::index) const COVERAGE: &str = concat!(
    "SELECT count(*),coalesce(sum(v.input_hash IS NOT NULL),0),coalesce(sum(v.input_hash IS \
        NULL AND f.chunk_id IS NOT NULL),0)\n            FROM chunks c JOIN pages p ON \
        p.slug=c.slug\n            LEFT JOIN vector_cache.vectors v ON \
        v.input_hash=c.input_hash AND v.fingerprint=:fingerprint\n            LEFT JOIN \
        embedding_failures f ON f.chunk_id=c.id AND f.fingerprint=:fingerprint WHERE ",
    filter!()
);
pub(super) const HYDRATE: &str = "SELECT \
    p.title,p.kind,c.text,c.heading,c.zone,c.verified,p.verified FROM chunks c JOIN pages p \
    ON p.slug=c.slug WHERE c.id=?1";
