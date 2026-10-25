use super::*;

fn mutate(record: &mut ledger::Record, defect: &str) {
    match defect {
        "revision" => record.revision = 1,
        "never_authorized" => record.never_authorized = false,
        "process" => record.process = Some(ProcessIdentity::current().unwrap()),
        "conversation" => {
            record.conversation = Some(("agent".into(), "thread".into(), "turn".into()));
        }
        "account_identity" => record.account_identity = Some("unexpected".into()),
        "quiescent" => record.quiescent = true,
        "confirmed" => record.confirmed = true,
        "authorized" => record.phase = ledger::ExecutionPhase::Authorized,
        "finished" => record.phase = ledger::ExecutionPhase::Finished,
        "generation" => record.generation = Uuid::new_v4(),
        "run" => record.run = Uuid::new_v4(),
        "owner" => record.owner.birth.push_str("-changed"),
        "boot" => record.boot.push_str("-changed"),
        "lock" => {
            let mut value = serde_json::to_value(&record.lock).unwrap();
            value["ino"] = 0.into();
            record.lock = serde_json::from_value(value).unwrap();
        }
        "class" => record.class = ledger::Class::Provider,
        _ => {}
    }
}

#[test]
fn every_nonpristine_or_mismatched_preparation_is_retained() {
    let _fork = crate::os::fork_guard().unwrap();
    for defect in [
        "none",
        "revision",
        "never_authorized",
        "process",
        "conversation",
        "account_identity",
        "quiescent",
        "confirmed",
        "authorized",
        "finished",
        "generation",
        "run",
        "owner",
        "boot",
        "lock",
        "class",
        "archive",
        "duplicate",
        "missing_committed",
        "wrong_committed_id",
    ] {
        let (_temporary, _state, mut operation, dir) = fixture();
        fail_preparation(&mut operation, true, Step::BeforeFileSync);
        let mut records = active(&dir);
        assert_eq!(records.len(), 1);
        let mut record = records.pop().unwrap();
        let path = dir.join(format!("executions/{}.json", record.id));
        mutate(&mut record, defect);
        write_json(&path, &record).unwrap();
        match defect {
            "archive" => {
                state::directory(&dir.join("execution-archive")).unwrap();
                fs::rename(
                    &path,
                    dir.join(format!("execution-archive/{}.json", record.id)),
                )
                .unwrap();
            }
            "duplicate" => {
                let mut second = record.clone();
                second.id = Uuid::new_v4();
                write_json(&dir.join(format!("executions/{}.json", second.id)), &second).unwrap();
            }
            "missing_committed" | "wrong_committed_id" => {
                let identity = dir.join("identity.json");
                let mut manifest: Manifest = read_json(&identity).unwrap();
                manifest.launch = Launch::Receipt(if defect == "missing_committed" {
                    record.id
                } else {
                    Uuid::new_v4()
                });
                write_json(&identity, &manifest).unwrap();
                if defect == "missing_committed" {
                    fs::remove_file(&path).unwrap();
                }
            }
            _ => {}
        }
        operation.finish().unwrap();
        assert_eq!(dir.exists(), defect != "none", "{defect}");
    }
}

#[test]
fn committed_pristine_receipt_can_finish_locally_but_its_archived_copy_cannot() {
    let _fork = crate::os::fork_guard().unwrap();
    for archived in [false, true] {
        let (_temporary, _state, mut operation, dir) = fixture();
        let receipt = operation.prepare_child(false, false).unwrap();
        let record = active(&dir).pop().unwrap();
        if archived {
            state::directory(&dir.join("execution-archive")).unwrap();
            fs::rename(
                dir.join(format!("executions/{}.json", record.id)),
                dir.join(format!("execution-archive/{}.json", record.id)),
            )
            .unwrap();
        }
        drop(receipt);
        operation.finish().unwrap();
        assert_eq!(dir.exists(), archived);
    }
}
