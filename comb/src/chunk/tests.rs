use super::*;
use crate::values::LinkKind;

fn words(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("w{i}")).collect()
}

#[test]
fn chunk_windows_overlap_and_stop_at_the_last_word() {
    let text = words(551).join(" ");
    let mut chunks = Chunker::new(&text, "A / B", Zone::Timeline, true);
    for (first, last) in [(0, 299), (250, 549), (500, 550)] {
        let chunk = chunks.next().unwrap();
        assert_eq!(chunk.text, words(551)[first..=last].join(" "));
        assert_eq!(chunk.heading, "A / B");
        assert_eq!(chunk.zone, Zone::Timeline);
        assert!(chunk.verified);
    }
    assert!(chunks.next().is_none());
    assert!(chunks.next().is_none());
    assert!(
        Chunker::new(" \n\t", "", Zone::Compiled, false)
            .next()
            .is_none()
    );
}

#[test]
fn paragraph_boundary_precedes_later_sentence_boundary() {
    let mut words = words(400);
    words[280].push('.');
    let text = format!("{}\n\n{}", words[..220].join(" "), words[220..].join(" "));
    let chunks: Vec<_> = Chunker::new(&text, "", Zone::Compiled, false).collect();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].text, words[..220].join(" "));
    assert_eq!(chunks[1].text, words[170..].join(" "));
    assert!(!chunks[0].verified);
}

#[test]
fn sentence_floor_and_final_window_keep_original_boundaries() {
    let mut words = words(400);
    words[249].push('.');
    words[250].push('!');
    let text = words.join(" ");
    let chunks: Vec<_> = Chunker::new(&text, "", Zone::Compiled, false).collect();
    assert_eq!(chunks[0].text, words[..251].join(" "));
    assert_eq!(chunks[1].text, words[201..].join(" "));
    let text = words[..300].join(" ");
    let chunks: Vec<_> = Chunker::new(&text, "", Zone::Compiled, false).collect();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, text);
}

#[test]
fn sparse_heading_paths_keep_sections_and_replace_siblings() {
    let text =
        "intro\n\n# One\n\nfirst\n\n### Three\n\nthird\n\n## Two\n\nsecond\n\n# Other\n\nlast";
    let chunks = split(text, Zone::Compiled, true);
    let actual: Vec<_> = chunks
        .iter()
        .map(|c| (c.heading.as_str(), c.text.as_str()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("", "intro"),
            ("One", "first"),
            ("One / Three", "third"),
            ("One / Two", "second"),
            ("Other", "last")
        ]
    );
}

#[test]
fn links_order_by_target_then_kind_and_deduplicate() {
    let mut page = crate::golden::page(crate::Actor::Human);
    page.timeline.clear();
    page.compiled = "[[concepts/z]] [[concepts/a]] [[concepts/a|label]] [a](a.md) [a again](a.md#part)\n\n```\n[[concepts/hidden]]\n```".into();
    let actual: Vec<_> = links(&"concepts/source".parse().unwrap(), &page)
        .into_iter()
        .map(|link| (link.target.to_string(), link.kind))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("concepts/a".into(), LinkKind::Markdown),
            ("concepts/a".into(), LinkKind::Slug),
            ("concepts/z".into(), LinkKind::Slug)
        ]
    );
}
