use super::*;
use serde_json::json;
fn row(id: &str, parent: Option<&str>, text: &str) -> Value {
    json!({"type":"user","uuid":id,"parentUuid":parent,"message":{"role":"user","content":text}})
}
fn rows(values: &[Value]) -> Vec<Dialogue> {
    decode(
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    )
    .unwrap()
}
#[test]
fn cutoff_is_proved_from_complete_dialogue_prefix_not_generated_answer() {
    let ids = [
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    ];
    let prefix = row(&ids[0], None, "prefix");
    let child = row(&ids[1], Some(&ids[0]), "child");
    let later = row(&ids[2], Some(&ids[0]), "later marker");
    let child_chain = rows(&[prefix.clone(), child.clone()]);
    extends(&rows(std::slice::from_ref(&prefix)), &child_chain).unwrap();
    assert!(extends(&rows(std::slice::from_ref(&prefix)), &child_chain[1..]).is_err());
    let mut edited = prefix.clone();
    edited["message"]["content"] = json!("changed prefix");
    assert!(extends(&rows(&[prefix]), &rows(&[edited, later])).is_err());
}

#[test]
fn copied_prefix_requires_connected_ancestry_without_unknown_parents_or_cycles() {
    let root = uuid::Uuid::new_v4().to_string();
    let child = uuid::Uuid::new_v4().to_string();
    let unknown = uuid::Uuid::new_v4().to_string();
    let prefix = row(&root, None, "copied source prefix");
    for parent in [None, Some(unknown.as_str()), Some(child.as_str())] {
        let fork = row(&child, parent, "acknowledged fork input");
        assert!(decode(format!("{prefix}\n{fork}").as_bytes()).is_err());
    }
    let forward_root = row(&root, Some(&child), "cycle root");
    let cycle_child = row(&child, Some(&root), "cycle child");
    assert!(decode(format!("{forward_root}\n{cycle_child}").as_bytes()).is_err());
}
#[test]
fn duplicate_ids_bad_parent_and_sidechains_do_not_make_a_proof() {
    let id = uuid::Uuid::new_v4().to_string();
    let value = row(&id, None, "input");
    assert!(decode(format!("{value}\n{value}").as_bytes()).is_err());
    for (key, val) in [
        ("parentUuid", json!("invalid")),
        ("isSidechain", json!(true)),
        ("message", json!(null)),
    ] {
        let mut bad = value.clone();
        bad[key] = val;
        assert!(decode(bad.to_string().as_bytes()).is_err());
    }
}
