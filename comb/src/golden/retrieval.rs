use crate::{
    Actor, Comb, Page, Result,
    disk::golden::seeded_comb,
    embedding::{self, Embedder, Space},
    golden,
    search::{Mode, QueryVector, Search},
};
use serde_json::json;
mod upgrade;

struct Deterministic {
    space: Space,
}
impl Deterministic {
    fn new() -> Self {
        Self {
            space: serde_json::from_value(json!({
                "backend":"fixture", "model":"deterministic", "revision":"fixed", "dimensions":3,
                "document_prefix":"doc: ",
                     "query_prefix":"query: ",
                     "preprocessing":"fixture",
                     "normalization":"l2-f32"
            }))
            .unwrap(),
        }
    }
}
impl Embedder for Deterministic {
    fn space(&self) -> &Space {
        &self.space
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        Ok(inputs
            .iter()
            .map(|input| {
                Ok(if input.contains("writer") {
                    vec![3.0, 4.0, 0.0]
                } else if input.contains("approval") {
                    vec![0.0, 4.0, 3.0]
                } else {
                    vec![0.0, 0.0, 1.0]
                })
            })
            .collect())
    }
}

fn corpus() -> Vec<(&'static str, Page)> {
    use std::fmt::Write as _;
    let mut long = String::new();
    for i in 0..780 {
        write!(long, "word{i}{} ", if i % 45 == 0 { ".\n\n" } else { "" }).unwrap();
    }
    let mut pages: Vec<_> = [
        (
            "concepts/writer",
            format!(
                "# Durable writer\nThe writer retains messages. \
            [approval](approval.md).\n\n{long}"
            ),
        ),
        (
            "concepts/approval",
            "# Human approval\nAn approval wakes an agent. \
            [[concepts/writer|Writer]]\n\n## Code\n```rust\n// [[ignored/link]]\nfn main() \
            {}\n```"
                .into(),
        ),
        (
            "concepts/cjk",
            "# 知識\n日本語の知識と証拠。\n\n## 第二\nUnicode café Ελληνικά Русский.".into(),
        ),
    ]
    .into_iter()
    .map(|(slug, body)| {
        let mut page = Page::parse(&format!(
            "---\ntitle: {}\ntype: concept\naliases: \
            [knowledge]\n---\n{body}",
            slug.rsplit('/').next().unwrap()
        ))
        .unwrap();
        golden::edit_record(&mut page.metadata, |raw| {
            raw["author"] = json!(Actor::Human);
            raw["created_at"] = json!(golden::AT);
            raw["updated_at"] = json!(golden::AT);
        });
        (slug, page)
    })
    .collect();
    let mut promoted = golden::page(Actor::Human);
    promoted.metadata.authored.title = "Promoted writer".into();
    promoted.compiled = "The writer has a reviewed summary.".into();
    promoted.timeline.truncate(1);
    promoted.timeline[0].text = "reviewedproof: measured writer durability.".into();
    golden::edit_record(&mut promoted.timeline[0].metadata, |raw| {
        raw["actor"] = json!(golden::actors()[1]);
    });
    promoted.promote(Actor::Human).unwrap();
    golden::edit_record(&mut promoted.metadata, |raw| {
        raw["promotion"]["at"] = json!(golden::AT);
        raw["promotion"]["evidence"]["at"] = json!(golden::AT);
    });
    let mut pending = promoted.timeline[0].clone();
    pending.metadata.authored.id = "23456789-1234-4234-8234-123456789abc".parse().unwrap();
    golden::edit_record(&mut pending.metadata, |raw| {
        raw["actor"] = json!(Actor::External);
        raw["consolidation"] = serde_json::Value::Null;
    });
    pending.text = "newproof: an unreviewed writer observation.".into();
    // New evidence precedes the authenticated reviewed suffix.
    promoted.timeline.insert(0, pending);
    assert_eq!(promoted.reviewed_start(), Some(1));
    pages.push(("concepts/promoted", promoted));
    pages
}

fn fill(comb: &mut Comb, model: &mut Deterministic) {
    comb.activate_embeddings(model.space(), false).unwrap();
    let mut scan = embedding::Scan::default();
    loop {
        let batch = comb
            .index()
            .unwrap()
            .embedding_jobs(&mut scan, embedding::BATCH_SIZE)
            .unwrap();
        if batch.jobs.is_empty() {
            break;
        }
        comb.apply_embeddings(embedding::run_batch(model, batch).unwrap())
            .unwrap();
    }
}

#[test]
fn unconstrained_links_require_rebuild_without_changing_pages_or_valid_vectors() {
    let pages = corpus();
    let (root, mut comb) = seeded_comb(&pages);
    // Direct index admission requires the canonical paths used by Disk.
    let root_path = root.path().canonicalize().unwrap();
    let mut model = Deterministic::new();
    fill(&mut comb, &mut model);
    let authored: Vec<_> = pages
        .iter()
        .map(|(slug, _)| {
            let path = root.path().join(format!("{slug}.md"));
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let unconstrained = crate::index::SCHEMA
        .lines()
        .find(|line| line.starts_with("CREATE TABLE links "))
        .unwrap()
        .replace(" CHECK(kind IN ('markdown','slug'))", "");
    comb.index
        .test_connection()
        .execute_batch(&format!(
            "DROP TABLE links; {unconstrained}
             CREATE INDEX backlinks_lookup ON links(target,source);
             INSERT INTO links VALUES('concepts/approval','concepts/writer','unknown');"
        ))
        .unwrap();
    drop(comb);
    let main = root_path.join(".comb/index.sqlite");
    let vectors = root_path.join(".comb/vectors.sqlite");
    let cached = std::fs::read(&vectors).unwrap();
    assert_eq!(
        crate::Index::open(&main).err().unwrap().code,
        crate::ErrorCode::UnsupportedIndex
    );
    assert!(crate::Index::needs_automatic_rebuild(&main).unwrap());

    let rebuilt = Comb::rebuild(root.path()).unwrap();
    let kind: crate::values::LinkKind = rebuilt
        .index
        .test_connection()
        .query_row(
            "SELECT kind FROM links WHERE source='concepts/approval' AND target='concepts/writer'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, crate::values::LinkKind::Slug);
    assert_eq!(
        rebuilt.index.active_space().unwrap().unwrap().1,
        model.space
    );
    assert_eq!(
        rebuilt
            .index
            .embedding_coverage(crate::search::Filter::default())
            .unwrap()
            .pending,
        0
    );
    drop(rebuilt);
    for (path, bytes) in authored {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(std::fs::read(vectors).unwrap(), cached);
}

#[test]
fn persisted_chunks_links_input_hashes_and_vector_bytes_are_stable() {
    let (_root, mut comb) = seeded_comb(&corpus());
    fill(&mut comb, &mut Deterministic::new());
    let mut statement = comb.index.test_connection().prepare("SELECT slug, ordinal, zone, heading, text, verified, input_hash FROM chunks ORDER BY slug,ordinal").unwrap();
    let chunks = statement
        .query_map([], |row| {
            Ok(json!({
                "slug":row.get::<_,
                    String>(0)?,
                     "ordinal":row.get::<_,
                    u32>(1)?,
                     "zone":row.get::<_,
                    String>(2)?,

                "heading":row.get::<_,
                    String>(3)?,
                     "text":row.get::<_,
                    String>(4)?,
                     "verified":row.get::<_,
                    bool>(5)?,
                     "input_hash":row.get::<_,
                    String>(6)?
            }))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    golden::json("chunks.json", &chunks);
    let mut statement = comb
        .index
        .test_connection()
        .prepare("SELECT source,target,kind FROM links ORDER BY source,target,kind")
        .unwrap();
    let links = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    golden::json("links.json", &links);
    let mut statement = comb
        .index
        .test_connection()
        .prepare("SELECT input_hash, hex(vector) FROM vector_cache.vectors ORDER BY input_hash")
        .unwrap();
    let vectors = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    golden::json("vectors.json", &vectors);
}

#[test]
fn ranking_and_scores_are_bit_identical_in_all_modes() {
    let (_root, mut comb) = seeded_comb(&corpus());
    let mut model = Deterministic::new();
    fill(&mut comb, &mut model);
    comb.put_markdown(
        &"concepts/pending".parse().unwrap(),
        "---\ntitle: Pending writer\n---\nwriter queued text-only.",
        None,
        &crate::store::Write::new(&Actor::Human, &"pending".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    for (suffix, query) in [
        ("", "writer"),
        ("-reviewed", "reviewedproof"),
        ("-unreviewed", "newproof"),
        ("-title", "Promoted writer"),
    ] {
        for (label, mode) in [
            ("text", Mode::Text),
            ("vector", Mode::Vector),
            ("hybrid", Mode::Hybrid),
        ] {
            record_search_case(&comb, &mut model, suffix, query, label, mode);
        }
    }
}

fn record_search_case(
    comb: &Comb,
    model: &mut Deterministic,
    suffix: &str,
    query: &str,
    label: &str,
    mode: Mode,
) {
    let request = Search {
        query: query.into(),
        ..Search::default()
    };
    let lease = comb
        .index()
        .unwrap()
        .embedding_lease(model.space())
        .unwrap();
    let query = embedding::run_query(model, lease, &request.query).unwrap();
    let result = comb
        .index()
        .unwrap()
        .retrieve(&request, mode, QueryVector::Vector(query))
        .unwrap();
    if suffix.is_empty() {
        assert_eq!(
            result
                .results
                .iter()
                .any(|hit| hit.slug == "concepts/pending".parse().unwrap()),
            mode != Mode::Vector
        );
    }
    if mode == Mode::Text && (suffix == "-reviewed" || suffix == "-unreviewed") {
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].zone, crate::values::Zone::Timeline);
        assert_eq!(result.results[0].unverified, suffix == "-unreviewed");
    }
    let bits = result
        .results
        .iter()
        .map(|hit| (hit.slug.clone(), hit.score.to_bits()))
        .collect::<Vec<_>>();
    golden::json(&format!("search-{label}{suffix}.json"), &result);
    golden::json(&format!("search-{label}{suffix}-bits.json"), &bits);
}
