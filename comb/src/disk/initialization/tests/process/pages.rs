use super::*;
use crate::{Actor, Comb, store::Mutation, test_support::write as fixture_write};

fn create(comb: &mut Comb) -> Mutation {
    comb.put_markdown(
        &"concepts/new".parse().unwrap(),
        "---\ntitle: Complete page\n---\nComplete content.",
        None,
        &fixture_write(&Actor::Human, "create-page", None).unwrap(),
    )
    .unwrap()
}

pub(super) fn exit_during_create(root: &Path, index: usize) -> ! {
    let mut comb = Comb::open(root).unwrap();
    let target = comb.disk.root.join("concepts/new.md");
    hooks::with(
        move |path, step| {
            if path == target && step == STEPS[index] {
                std::process::exit(73);
            }
            Ok(())
        },
        || create(&mut comb),
    );
    panic!("page creation death boundary not reached");
}

#[test]
fn page_creation_process_death_leaves_absence_or_complete_page_and_exact_replay() {
    for index in 0..STEPS.len() {
        let root = tempfile::tempdir().unwrap();
        drop(Comb::init(root.path()).unwrap());
        crash(root.path(), "page-create", index);
        let target = root.path().join("concepts/new.md");
        let published = index >= 4;
        if published {
            let raw = fs::read_to_string(&target).unwrap();
            assert_eq!(
                crate::Page::parse(&raw).unwrap().compiled,
                "Complete content."
            );
        } else {
            assert!(!target.exists());
        }
        assert!(root.path().join(".comb/intent.json").exists());
        assert!(preparation(&root.path().join("concepts")).is_empty());
        let stages = preparation(&root.path().join(".comb"));
        assert_eq!(stages.len(), usize::from(!published));
        for stage in &stages {
            assert!(hive_setup_support::names::comb_admin_preparation_name(
                stage.file_name().unwrap()
            ));
            assert_eq!(fs::metadata(stage).unwrap().mode() & 0o777, 0o600);
        }
        let mut comb = Comb::open(root.path()).unwrap();
        assert!(stages.iter().all(|stage| !stage.exists()));
        assert!(!root.path().join(".comb/intent.json").exists());
        assert_eq!(create(&mut comb).replayed, published);
        assert!(create(&mut comb).replayed);
        assert_eq!(
            comb.get(&"concepts/new".parse().unwrap())
                .unwrap()
                .page
                .compiled,
            "Complete content."
        );
    }
}
