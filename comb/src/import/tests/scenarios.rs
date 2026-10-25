use super::*;

#[test]
fn resume_keeps_batch_ids_receipts_and_completed_edits_or_deletions() {
    let mut fixture = Fixture::new();
    let progress = fixture.pending(2);
    let journal = fixture.journal();
    let a: Slug = "a".parse().unwrap();
    let b: Slug = "b".parse().unwrap();
    let edited = fixture
        .comb
        .append(&a, "Independent fact", "test", None, None, &write("edit"))
        .unwrap();
    let b_hash = fixture.comb.get(&b).unwrap().hash;
    fixture.comb.remove(&b, &b_hash, &write("remove")).unwrap();
    assert_eq!(fixture.journal(), journal);
    assert_eq!(fixture.status().completed, 2);
    let mut fixture = fixture.reopen();
    let report = fixture.apply().unwrap();
    assert_eq!(report.batch_id, progress.batch_id);
    assert_eq!(
        report.completed,
        ["a", "b", "c"].map(|s| s.parse::<Slug>().unwrap())
    );
    assert_eq!(fixture.comb.get(&a).unwrap().hash, edited.hash);
    assert!(fixture.comb.get(&b).unwrap_err().is_not_found());
    // A lost reply is replayed without reconstructing a deleted destination.
    let complete = fixture.journal();
    assert_eq!(fixture.apply().unwrap(), report);
    assert_eq!(fixture.journal(), complete);
    assert_eq!(fixture.status().state, ImportState::Completed);
}

#[test]
fn unfinished_conflict_preflights_all_pages_before_remaining_writes() {
    let mut fixture = Fixture::new();
    let progress = fixture.pending(1);
    let edited = fixture
        .comb
        .put_markdown(
            &"c".parse().unwrap(),
            "---\ntitle: Independent\n---\nKeep this",
            None,
            &write("independent"),
        )
        .unwrap();
    let before = fixture.journal();
    let error = fixture.apply().unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.file_committed);
    assert_eq!(error.import_progress.as_ref().unwrap().completed, 1);
    assert!(!fixture.page("b").exists());
    assert_eq!(fixture.comb.get(&edited.slug).unwrap().hash, edited.hash);
    assert_eq!(fixture.journal(), before);
    let abandoned = fixture.comb.abandon_import(&progress.batch_id).unwrap();
    assert_eq!(
        (abandoned.state, abandoned.completed, abandoned.unfinished),
        (ImportState::Abandoned, 1, 2)
    );
    assert_eq!(
        fixture.comb.abandon_import(&progress.batch_id).unwrap(),
        abandoned
    );
    let next = fixture.apply().unwrap();
    assert_ne!(next.batch_id, progress.batch_id);
    assert_eq!(
        fixture
            .comb
            .abandon_import(&progress.batch_id)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(fixture.status().batch_id, next.batch_id);
}

#[test]
fn changed_source_never_silently_uses_stored_markdown() {
    for change in 0..6 {
        let mut fixture = Fixture::new();
        let pending = fixture.pending(1);
        let before = fixture.journal();
        let mut prefix = None;
        match change {
            0 => fs::write(fixture.source.path().join("a.md"), "changed").unwrap(),
            1 => fs::write(fixture.source.path().join("d.md"), "added").unwrap(),
            2 => fs::remove_file(fixture.source.path().join("b.md")).unwrap(),
            3 => fs::remove_dir_all(fixture.source.path()).unwrap(),
            4 => prefix = Some("other".parse().unwrap()),
            _ => {
                let replacement = tempfile::tempdir().unwrap();
                for name in ["a", "b", "c"] {
                    fs::write(replacement.path().join(format!("{name}.md")), name).unwrap();
                }
                fixture.source = replacement;
            }
        }
        let error = fixture
            .comb
            .import(fixture.source.path(), prefix.as_ref())
            .unwrap_err();
        assert!(error.file_committed, "{change}");
        assert_eq!(error.import_progress.unwrap().batch_id, pending.batch_id);
        assert_eq!(fixture.journal(), before);
        assert!(!fixture.page("b").exists());
        let status = fixture.status();
        assert_eq!(status.batch_id, pending.batch_id);
        fixture.comb.abandon_import(&status.batch_id).unwrap();
        assert!(fixture.page("a").exists());
    }
}

#[test]
fn empty_input_and_latest_batch_replay_are_explicit() {
    let mut fixture = Fixture::new();
    assert!(fixture.comb.import_status().unwrap().current.is_none());
    assert!(fixture.comb.abandon_import(&ImportId::generate()).is_err());
    let empty = tempfile::tempdir().unwrap();
    let first = fixture.comb.import(empty.path(), None).unwrap();
    assert_eq!(first.total, 0);
    assert!(first.completed.is_empty());
    assert_eq!(fixture.comb.import(empty.path(), None).unwrap(), first);
    fixture.apply().unwrap();
    let third = fixture.comb.import(empty.path(), None).unwrap();
    assert_ne!(first.batch_id, third.batch_id);
    assert_eq!(
        fixture.comb.abandon_import(&third.batch_id).unwrap().state,
        ImportState::Completed
    );
}

#[test]
fn inputs_overlapping_destinations_are_rejected_but_nested_source_is_allowed() {
    let mut fixture = Fixture::new();
    fs::write(fixture.root.path().join("a.md"), "a").unwrap();
    let error = fixture.comb.import(fixture.root.path(), None).unwrap_err();
    assert_eq!(error.message, Error::import_overlap().message);
    assert!(!fixture.root.path().join(".comb/import.json").exists());
    let nested = fixture.root.path().join("source");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("note.md"), "Nested source").unwrap();
    fixture.comb.import(&nested, None).unwrap();
    assert!(fixture.page("note").exists());
    assert_eq!(
        fs::read_to_string(nested.join("note.md")).unwrap(),
        "Nested source"
    );
}
