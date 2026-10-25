use super::*;

#[test]
#[ignore = "subprocess fixture invoked by record_death_boundaries"]
fn record_death_child() {
    let path = PathBuf::from(std::env::var_os("COMB_RECORD_DEATH_PATH").unwrap());
    let phase = std::env::var("COMB_RECORD_DEATH_PHASE").unwrap();
    let once = std::env::var("COMB_RECORD_DEATH_ONCE").unwrap() == "true";
    let _guard = hooks::install(move |_, step| {
        if format!("{step:?}") == phase {
            std::process::exit(71);
        }
        Ok(())
    });
    if once {
        create_record(&path, b"42").unwrap();
    } else {
        write_json(&path, &42).unwrap();
    }
    panic!("death boundary was not reached");
}

#[test]
fn record_death_boundaries() {
    for once in [false, true] {
        for phase in [
            Step::BeforeFileSync,
            Step::AfterFileSync,
            Step::BeforePublish,
            Step::AfterPublish,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("record.json");
            if !once {
                write_json(&path, &37).unwrap();
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "provision::files::publication::tests::process::record_death_child",
                    "--ignored",
                ])
                .env("COMB_RECORD_DEATH_PATH", &path)
                .env("COMB_RECORD_DEATH_PHASE", format!("{phase:?}"))
                .env("COMB_RECORD_DEATH_ONCE", once.to_string())
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(71), "{output:?}");
            let published = matches!(
                phase,
                Step::AfterPublish | Step::BeforeDirectorySync | Step::AfterDirectorySync
            );
            if published || !once {
                assert_eq!(
                    crate::provision::files::json::<u64>(&path).unwrap(),
                    if published { 42 } else { 37 }
                );
            } else {
                assert!(!path.exists());
            }
            write_json(&path, &42).unwrap();
        }
    }
}

#[test]
fn readers_acknowledge_complete_json_during_create_and_replace() {
    for once in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("record.json");
        if !once {
            write_json(&path, &37).unwrap();
        }
        let (request, requests) = std::sync::mpsc::channel();
        let (answer, answers) = std::sync::mpsc::channel();
        let read_path = path.clone();
        let reader = std::thread::spawn(move || {
            while requests.recv().is_ok() {
                let value = if read_path.exists() {
                    Some(crate::provision::files::json::<u64>(&read_path).unwrap())
                } else {
                    None
                };
                answer.send(value).unwrap();
            }
        });
        let guard = hooks::install(move |_, step| {
            if matches!(step, Step::BeforePublish | Step::AfterPublish) {
                for _ in 0..10 {
                    request.send(()).unwrap();
                    let got = answers
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    assert_eq!(
                        got,
                        if step == Step::AfterPublish {
                            Some(42)
                        } else if once {
                            None
                        } else {
                            Some(37)
                        }
                    );
                }
            }
            Ok(())
        });
        if once {
            create_record(&path, b"42").unwrap();
        } else {
            write_json(&path, &42).unwrap();
        }
        drop(guard);
        reader.join().unwrap();
    }
}
