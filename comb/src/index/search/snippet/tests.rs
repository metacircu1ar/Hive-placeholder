use super::snippet;

#[test]
fn unicode_matches_use_byte_offsets_and_deduplicate_query_terms() {
    let text = "Résumé résumé 日本語";
    let result = snippet(text, "RÉSUMÉ résumé 日本語");
    assert_eq!(result.text, text);
    assert_eq!(result.highlights, [[0, 8], [9, 17], [18, 27]]);
    for [start, end] in result.highlights {
        assert!(result.text.is_char_boundary(start));
        assert!(result.text.is_char_boundary(end));
    }
}

#[test]
fn window_keeps_eighty_prior_characters_and_clips_crossing_matches() {
    let text = format!(
        "{}needle{}tail{}",
        "α".repeat(100),
        "β".repeat(393),
        "γ".repeat(100)
    );
    let result = snippet(&text, "needle needle tail");
    assert_eq!(
        result.text,
        format!("{}needle{}t", "α".repeat(80), "β".repeat(393))
    );
    assert_eq!(result.text.chars().count(), 480);
    assert_eq!(result.highlights, [[160, 166]]);
}

#[test]
fn missing_and_empty_terms_keep_the_original_start_window() {
    let result = snippet(&"é".repeat(600), "absent");
    assert_eq!(result.text, "é".repeat(480));
    assert!(result.highlights.is_empty());
    let result = snippet("", "query");
    assert!(result.text.is_empty());
    assert!(result.highlights.is_empty());
    let result = snippet("visible", "  ");
    assert_eq!(result.text, "visible");
    assert!(result.highlights.is_empty());
}

#[test]
fn lowercase_expansion_keeps_the_original_character_window_rule() {
    // Lowercased İ has two characters; the one-character source has no window
    // of that size. Preserve this behavior instead of introducing normalization.
    let result = snippet("İ", "İ");
    assert_eq!(result.text, "İ");
    assert!(result.highlights.is_empty());
}
