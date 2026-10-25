#![allow(clippy::unwrap_used)]
use super::*;
use std::fs;

#[test]
fn bundled_skill_metadata_matches_product_and_hosted_catalog() {
    for text in [
        include_str!("../../../../skills/hive/SKILL.md"),
        include_str!("../../../../skills/comb-curator/SKILL.md"),
    ] {
        let frontmatter = text
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0;
        let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(frontmatter).unwrap();
        assert_eq!(
            parsed["metadata"]["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            parsed["metadata"]["hive-agent-catalog"].as_str(),
            Some(
                crate::compatibility::AGENT_CATALOG
                    .current
                    .to_string()
                    .as_str()
            )
        );
    }
}

#[test]
fn installed_link_closure_updates_owned_files_preserves_edits_and_drops_removed_links() {
    let dir = tempfile::tempdir().unwrap();
    let product = dir.path().canonicalize().unwrap().join("share/hive");
    for name in ["hive", "comb-curator"] {
        fs::create_dir_all(product.join(format!("skills/{name}"))).unwrap();
        fs::write(
            product.join(format!("skills/{name}/SKILL.md")),
            "# Guide\n[CLI](../../docs/commands.md)\n",
        )
        .unwrap();
    }
    fs::create_dir_all(product.join("docs")).unwrap();
    fs::write(product.join("docs/commands.md"), "First CLI\n").unwrap();
    let destination = dir.path().canonicalize().unwrap().join("user/hive");
    let make = || {
        owned::Group::inspect(
            destination.clone(),
            desired("hive", &closure::bundle(&product).unwrap()).unwrap(),
        )
        .unwrap()
    };
    assert!(make().apply().unwrap());
    assert!(!make().apply().unwrap());
    assert!(
        fs::read_to_string(destination.join("SKILL.md"))
            .unwrap()
            .contains("bundle/docs/commands.md")
    );
    fs::write(product.join("docs/commands.md"), "New CLI\n").unwrap();
    assert!(
        owned::stale(
            &destination,
            &desired("hive", &closure::bundle(&product).unwrap()).unwrap()
        )
        .unwrap()
    );
    assert!(make().apply().unwrap());
    assert_eq!(
        fs::read_to_string(destination.join("bundle/docs/commands.md")).unwrap(),
        "New CLI\n"
    );
    fs::write(destination.join("bundle/docs/commands.md"), "user edit\n").unwrap();
    let wanted = desired("hive", &closure::bundle(&product).unwrap()).unwrap();
    assert!(owned::Group::inspect(destination.clone(), wanted).is_err());
    fs::write(destination.join("bundle/docs/commands.md"), "New CLI\n").unwrap();
    for name in ["hive", "comb-curator"] {
        fs::write(
            product.join(format!("skills/{name}/SKILL.md")),
            "# New guide\n",
        )
        .unwrap();
    }
    fs::remove_file(product.join("docs/commands.md")).unwrap();
    assert!(make().apply().unwrap());
    assert!(!destination.join("bundle/docs/commands.md").exists());
    assert!(!make().apply().unwrap());
}

#[test]
fn link_closure_refuses_escape_symlink_and_ignores_fenced_examples() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for name in ["hive", "comb-curator"] {
        fs::create_dir_all(root.join(format!("skills/{name}"))).unwrap();
        fs::write(
            root.join(format!("skills/{name}/SKILL.md")),
            "```md\n[example](missing.md)\n```\n",
        )
        .unwrap();
    }
    assert_eq!(closure::bundle(&root).unwrap().len(), 2);
    let guide = root.join("skills/hive/SKILL.md");
    fs::write(&guide, "[bad](../../../outside)\n").unwrap();
    assert!(closure::bundle(&root).is_err());
    fs::write(&guide, "[bad](../../link)\n").unwrap();
    std::os::unix::fs::symlink("skills/comb-curator/SKILL.md", root.join("link")).unwrap();
    assert!(closure::bundle(&root).is_err());
}

#[test]
fn interrupted_owned_update_resumes_and_removal_refuses_modified_files() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("skill");
    let first = BTreeMap::from([
        (PathBuf::from("SKILL.md"), b"old guide".to_vec()),
        (PathBuf::from("bundle/doc.md"), b"old doc".to_vec()),
    ]);
    owned::Group::inspect(root.clone(), first)
        .unwrap()
        .apply()
        .unwrap();
    let next = BTreeMap::from([
        (PathBuf::from("SKILL.md"), b"new guide".to_vec()),
        (PathBuf::from("bundle/doc.md"), b"new doc".to_vec()),
    ]);
    let record = root.join(".hive-owned.json");
    let mut intent: serde_json::Value =
        serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    intent["pending"] = serde_json::to_value(
        next.iter()
            .map(|(p, b)| (p.clone(), format!("{:x}", Sha256::digest(b))))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap();
    fs::write(&record, serde_json::to_vec(&intent).unwrap()).unwrap();
    fs::write(root.join("SKILL.md"), "new guide").unwrap();
    assert!(
        owned::Group::inspect(root.clone(), next)
            .unwrap()
            .apply()
            .unwrap()
    );
    let reduced = BTreeMap::from([(PathBuf::from("SKILL.md"), b"new guide".to_vec())]);
    fs::write(root.join("bundle/doc.md"), "user revised").unwrap();
    assert!(owned::Group::inspect(root.clone(), reduced.clone()).is_err());
    fs::write(root.join("bundle/doc.md"), "new doc").unwrap();
    owned::Group::inspect(root.clone(), reduced)
        .unwrap()
        .apply()
        .unwrap();
    assert!(!root.join("bundle/doc.md").exists());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(record).unwrap()).unwrap()["pending"]
            .is_null()
    );
}

#[test]
fn group_writable_skill_directories_allow_updates_and_recovery_but_not_writable_files() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("skill");
    fs::create_dir_all(root.join("bundle")).unwrap();
    for parent in [dir.path(), &root, &root.join("bundle")] {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o775)).unwrap();
    }
    let make = |body: &[u8]| {
        owned::Group::inspect(
            root.clone(),
            BTreeMap::from([
                (PathBuf::from("SKILL.md"), body.to_vec()),
                (PathBuf::from("bundle/doc.md"), body.to_vec()),
            ]),
        )
    };
    if rustix::process::geteuid().is_root() {
        assert!(make(b"first").unwrap().apply().is_err());
        return;
    }
    assert!(make(b"first").unwrap().apply().unwrap());
    assert!(!make(b"first").unwrap().apply().unwrap());
    assert!(make(b"second").unwrap().apply().unwrap());
    assert!(!make(b"second").unwrap().apply().unwrap());
    assert_eq!(fs::read(root.join("bundle/doc.md")).unwrap(), b"second");
    for parent in [dir.path(), &root, &root.join("bundle")] {
        assert_eq!(fs::metadata(parent).unwrap().mode() & 0o777, 0o775);
    }
    fs::set_permissions(root.join("SKILL.md"), fs::Permissions::from_mode(0o664)).unwrap();
    assert!(make(b"third").is_err());
}
