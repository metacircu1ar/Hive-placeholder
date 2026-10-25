use super::*;
use crate::reader::{Filter, Reader};
use atomic_fs::{Append, Durability, TailPolicy};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
};

#[test]
fn follow_pages_bound_matching_records_without_skipping_the_remainder() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    for i in 0..3000 {
        writeln!(writer, "{{\"event\":\"item\",\"sequence\":{i}}}").unwrap();
    }
    let reader = Reader::open(&path).unwrap();
    let tail = reader.tail(3, &Filter::default()).unwrap();
    assert_eq!(tail.records[0]["sequence"], 2997);
    let first = reader.after(None, &Filter::default()).unwrap();
    assert_eq!(first.records.len(), 2000);
    assert!(first.more);
    let second = reader
        .after(first.cursor.as_ref(), &Filter::default())
        .unwrap();
    assert_eq!(second.records.len(), 1000);
    assert_eq!(second.records[0]["sequence"], 2000);
    assert!(!second.more);
}

#[test]
fn continuous_rotation_preserves_every_retained_record_exactly_once() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = Append::open(
        &path,
        TailPolicy::rotate(std::num::NonZeroUsize::new(63).unwrap()),
        Durability::AtomicVisible,
    )
    .unwrap();
    let reader = Reader::open(&path).unwrap();
    let mut cursor = reader.tail(1, &Filter::default()).unwrap().cursor;
    let producer = std::thread::spawn(move || {
        for n in 1..=200 {
            if n % 5 == 0 {
                writer.rotate().unwrap();
            }
            writer
                .append_line(
                    (format!("{{\"event\":\"item\",\"sequence\":{n}}}\n").as_bytes())
                        .strip_suffix(b"\n")
                        .unwrap(),
                )
                .unwrap();
            std::thread::yield_now();
        }
    });
    let filter = Filter {
        event: Some("item".into()),
        ..Filter::default()
    };
    let mut observed = Vec::new();
    let until = Instant::now() + Duration::from_secs(10);
    while observed.len() < 200 {
        assert!(
            Instant::now() < until,
            "retained records missing: {}",
            observed.len()
        );
        match reader.after(cursor.as_ref(), &filter) {
            Ok(page) => {
                assert!(!page.warnings.pruned);
                observed.extend(page.records.iter().map(|v| v["sequence"].as_u64().unwrap()));
                cursor = page.cursor;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
            Err(error) => panic!("follow: {error}"),
        }
        std::thread::yield_now();
    }
    producer.join().unwrap();
    assert_eq!(observed, (1..=200).collect::<Vec<_>>());
}

#[test]
fn reader_follows_rotation_without_repeating_records_and_reports_pruned_cursors() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("hive-daemon.jsonl");
    let mut writer = Append::open(
        &path,
        TailPolicy::rotate(std::num::NonZeroUsize::new(2).unwrap()),
        Durability::AtomicVisible,
    )
    .unwrap();
    let record = |n| format!("{{\"event\":\"test.item\",\"sequence\":{n},\"level\":\"info\"}}\n");
    writer
        .append_line((record(1).as_bytes()).strip_suffix(b"\n").unwrap())
        .unwrap();
    let reader = Reader::open(&path).unwrap();
    let filter = Filter {
        event: Some("test.item".into()),
        ..Filter::default()
    };
    let first = reader.tail(100, &filter).unwrap();
    assert_eq!(first.records[0]["sequence"], 1);
    writer
        .append_line((record(2).as_bytes()).strip_suffix(b"\n").unwrap())
        .unwrap();
    writer.rotate().unwrap();
    writer
        .append_line((record(3).as_bytes()).strip_suffix(b"\n").unwrap())
        .unwrap();
    let next = reader.after(first.cursor.as_ref(), &filter).unwrap();
    assert!(!next.warnings.pruned);
    assert_eq!(
        next.records
            .iter()
            .map(|r| r["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(
        reader
            .after(next.cursor.as_ref(), &filter)
            .unwrap()
            .records
            .is_empty()
    );
    for n in 4..7 {
        writer.rotate().unwrap();
        writer
            .append_line((record(n).as_bytes()).strip_suffix(b"\n").unwrap())
            .unwrap();
    }
    let pruned = reader.after(first.cursor.as_ref(), &filter).unwrap();
    assert!(pruned.warnings.pruned);
    assert_eq!(pruned.records.back().unwrap()["sequence"], 6);
    let tail = reader.tail(2, &filter).unwrap();
    assert_eq!(tail.records.len(), 2);
    assert_eq!(tail.records[0]["sequence"], 5);
}

#[test]
fn reader_withholds_incomplete_records_then_delivers_them_once() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    writer
        .write_all(b"{\"event\":\"test.item\",\"level\":\"warn\",\"agent_id\":\"a\"}\n{\"event\":")
        .unwrap();
    let reader = Reader::open(&path).unwrap();
    let filter = Filter {
        level: Some(tracing::Level::WARN),
        agent_id: Some("a".into()),
        ..Filter::default()
    };
    let first = reader.tail(20, &filter).unwrap();
    assert_eq!(first.records.len(), 1);
    assert!(first.warnings.incomplete);
    writer
        .write_all(b"\"test.item\",\"level\":\"error\",\"agent_id\":\"a\"}\nnot-json\n")
        .unwrap();
    let next = reader.after(first.cursor.as_ref(), &filter).unwrap();
    assert_eq!(next.records.len(), 1);
    assert_eq!(next.records[0]["level"], "error");
    assert!(!next.warnings.incomplete);
    assert_eq!(next.warnings.corrupt, 1);
    assert!(
        reader
            .after(next.cursor.as_ref(), &filter)
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn reader_refuses_symlinks_foreign_modes_and_non_regular_entries_without_creation() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("events.jsonl");
    let absent = directory.path().join("absent/events.jsonl");
    assert!(Reader::open(&absent).is_err());
    assert!(!absent.parent().unwrap().exists());
    fs::write(&path, "{}\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let reader = Reader::open(&path).unwrap();
    assert!(reader.tail(10, &Filter::default()).is_err());
    fs::remove_file(&path).unwrap();
    symlink("sensitive", &path).unwrap();
    assert!(reader.tail(10, &Filter::default()).is_err());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(reader.tail(10, &Filter::default()).is_err());
}

#[test]
fn oversized_unterminated_record_cannot_spin_follow_forever() {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    writer
        .write_all(&vec![
            b'x';
            usize::try_from(crate::reader::PAGE_BYTES).unwrap()
                + 100
        ])
        .unwrap();
    let reader = Reader::open(&path).unwrap();
    let first = reader.after(None, &Filter::default()).unwrap();
    assert!(first.more);
    assert_eq!(first.warnings.corrupt, 1);
    let second = reader
        .after(first.cursor.as_ref(), &Filter::default())
        .unwrap();
    assert!(!second.more);
    writer.write_all(b"\n{\"event\":\"test.valid\"}\n").unwrap();
    let last = reader
        .after(second.cursor.as_ref(), &Filter::default())
        .unwrap();
    assert_eq!(last.records.len(), 1);
    assert_eq!(last.records[0]["event"], "test.valid");
}
