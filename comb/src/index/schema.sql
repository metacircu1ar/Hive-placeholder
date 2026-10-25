CREATE TABLE pages (
 slug TEXT PRIMARY KEY, title TEXT NOT NULL, title_key TEXT NOT NULL, kind TEXT NOT NULL,
 content_hash TEXT NOT NULL, verified INTEGER NOT NULL, external INTEGER NOT NULL,
 updated_at TEXT NOT NULL, compiled TEXT NOT NULL, page_json TEXT NOT NULL,
 inbox_pending INTEGER NOT NULL CHECK(inbox_pending IN (0,1)),
 unresolved_proposal INTEGER NOT NULL CHECK(unresolved_proposal IN (0,1))
);
CREATE TABLE quarantined_pages (slug TEXT PRIMARY KEY, code TEXT NOT NULL);
CREATE TABLE tags (slug TEXT NOT NULL REFERENCES pages(slug) ON DELETE CASCADE, tag TEXT NOT NULL, PRIMARY KEY(slug,tag));
CREATE INDEX tags_lookup ON tags(tag,slug);
CREATE TABLE aliases (slug TEXT NOT NULL REFERENCES pages(slug) ON DELETE CASCADE, alias TEXT NOT NULL, PRIMARY KEY(slug,alias));
CREATE INDEX aliases_lookup ON aliases(alias,slug);
CREATE TABLE links (source TEXT NOT NULL REFERENCES pages(slug) ON DELETE CASCADE, target TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('markdown','slug')), PRIMARY KEY(source,target,kind));
CREATE INDEX backlinks_lookup ON links(target,source);
CREATE TABLE chunks (
 id INTEGER PRIMARY KEY, slug TEXT NOT NULL REFERENCES pages(slug) ON DELETE CASCADE,
 ordinal INTEGER NOT NULL, zone TEXT NOT NULL, heading TEXT NOT NULL,
 text TEXT NOT NULL, verified INTEGER NOT NULL, input_hash TEXT NOT NULL, instance TEXT NOT NULL,
 UNIQUE(slug,ordinal)
);
CREATE INDEX chunks_input_hash ON chunks(input_hash);
CREATE VIRTUAL TABLE chunk_fts USING fts5(title,heading,text,slug UNINDEXED,content='',contentless_delete=1,tokenize='unicode61');
CREATE VIRTUAL TABLE identity_fts USING fts5(slug,title,aliases,tokenize='trigram');
CREATE TABLE embedding_space (singleton INTEGER PRIMARY KEY CHECK(singleton=1), fingerprint TEXT NOT NULL, activation TEXT NOT NULL, specification TEXT NOT NULL);
CREATE TABLE embedding_failures (chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE, fingerprint TEXT NOT NULL, error TEXT NOT NULL);
