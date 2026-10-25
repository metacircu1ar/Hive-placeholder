use super::*;

#[test]
fn import_and_inbox_health_do_not_require_a_curator_subsystem() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(
        source.path().join("guide.md"),
        "# Existing Guide\n\nKnowledge.",
    )
    .unwrap();
    assert_eq!(
        comb.import(source.path(), Some(&"operations".parse().unwrap()))
            .unwrap()
            .completed,
        vec!["operations/guide".parse::<Slug>().unwrap()]
    );
    assert!(
        !comb
            .get(&"operations/guide".parse().unwrap())
            .unwrap()
            .unverified
    );
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    let note = comb
        .capture(
            "Investigate synchronization",
            &fixture_write(&actor, "note", None).unwrap(),
        )
        .unwrap();
    let mut resumed = actor.clone();
    if let Actor::Agent { turn, .. } = &mut resumed {
        *turn = "next-turn".into();
    }
    assert!(
        comb.capture(
            "Investigate synchronization",
            &fixture_write(&resumed, "note", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert!(comb.get(&note.slug).unwrap().unverified);
    assert_eq!(comb.doctor().unwrap().stats.inbox, 1);
}

#[test]
fn private_state_is_ignored_and_doctor_detects_git_exposure() {
    let (root, comb) = TestComb::new().unwrap().into_parts();
    let ignore = root.path().join(".comb/.gitignore");
    assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "*\n");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q"]).status.success());
    assert!(
        git(&["check-ignore", "-q", ".comb/identity.json"])
            .status
            .success()
    );
    std::fs::remove_file(&ignore).unwrap();
    assert!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::PrivateStateUnprotected)
    );
    crate::disk::atomic(&ignore, b"*\n", &crate::disk::System).unwrap();
    assert!(git(&["add", "-f", ".comb/identity.json"]).status.success());
    assert!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::PrivateStateTracked)
    );
}

#[test]
fn doctor_checks_missing_links_beyond_the_public_links_page() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/source".parse().unwrap(),
        &markdown("Source"),
        None,
        &fixture_write(&Actor::Human, "source", None).unwrap(),
    )
    .unwrap();
    // Populate the derived graph directly to isolate the diagnostic, avoiding
    // 1000 irrelevant durable-file writes in this regression.
    comb.index.seed_missing_link_beyond_page().unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .links(&"concepts/source".parse().unwrap(), false)
            .unwrap()
            .len(),
        1000
    );
    let report = comb.doctor().unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::BrokenLink && f.detail == "concepts/zz-missing")
    );
    assert!(!report.findings_truncated);
}

#[test]
fn doctor_bounds_findings_without_claiming_an_exact_page_is_truncated() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    // The exact-cap case requires a successful tracking check, not an extra
    // "unchecked" finding from running doctor outside a Git repository.
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    comb.put_markdown(
        &"concepts/source".parse().unwrap(),
        &markdown("# Source"),
        None,
        &fixture_write(&Actor::Human, "source", None).unwrap(),
    )
    .unwrap();
    comb.index.seed_finding_limit_links().unwrap();
    let report = comb.doctor().unwrap();
    assert_eq!(report.findings.len(), 1024);
    assert!(!report.findings_truncated);
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.kind == FindingKind::BrokenLink)
    );

    // A late private-state diagnostic must not overflow the public page.
    let ignore = root.path().join(".comb/.gitignore");
    std::fs::remove_file(&ignore).unwrap();
    let report = comb.doctor().unwrap();
    assert_eq!(report.findings.len(), 1024);
    assert!(report.findings_truncated);

    // Reaching the cap must not skip later ownership checks or hide errors.
    std::os::unix::fs::symlink(root.path().join("concepts/source.md"), ignore).unwrap();
    assert!(comb.doctor().is_err());
}

#[test]
fn bare_standard_prefixes_do_not_enter_descendant_backlogs() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    for slug in [
        "inbox",
        "inbox/child",
        "review",
        "review/child",
        "inboxish/child",
        "custom/child",
    ] {
        comb.put_markdown(
            &slug.parse().unwrap(),
            &markdown("Prefix policy"),
            None,
            &fixture_write(&Actor::Human, slug, None).unwrap(),
        )
        .unwrap();
    }
    let stats = comb.stats().unwrap();
    assert_eq!(stats.inbox, 1);
    assert_eq!(stats.unresolved_proposals, 1);
    let unknown: BTreeSet<_> = comb
        .doctor()
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.kind == FindingKind::UnknownPrefix)
        .map(|finding| finding.slug)
        .collect();
    assert_eq!(
        unknown,
        BTreeSet::from(["custom/child".into(), "inboxish/child".into()])
    );
}
