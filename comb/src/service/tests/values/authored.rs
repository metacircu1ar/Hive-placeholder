use super::*;

fn cases() -> Vec<InvalidOperation> {
    let hash = "a".repeat(64);
    [
        ("comb_put", json!({"slug":"concepts/payload","markdown":"PRIVATE invalid"}), "markdown"),
        ("comb_capture", json!({"text":" \n\t"}), "text"),
        ("comb_capture", json!({"text":"PRIVATE\0text"}), "text"),
        ("comb_append", json!({"slug":"concepts/payload","text":" ","source":"source"}), "text"),
        ("comb_append", json!({"slug":"concepts/payload","text":"text","source":""}), "source"),
        ("comb_append", json!({"slug":"concepts/payload","text":"text","source":"source","summary":"summary"}), "expected_hash"),
        ("comb_decide", json!({"slug":"concepts/payload","expected_hash":hash,"decision":"accept","reason":"reason"}), "slug"),
        ("comb_decide", json!({"slug":"review/proposal","expected_hash":hash,"decision":"accept","reason":" "}), "reason"),
        ("comb_consolidate", json!({"note":"concepts/payload","target":"concepts/target","note_hash":hash,"target_hash":hash,"summary":"summary","run":"run"}), "note"),
        ("comb_consolidate", json!({"note":"inbox/source","target":"review/proposal","note_hash":hash,"target_hash":hash,"summary":"summary","run":"run"}), "target"),
        ("comb_consolidate", json!({"note":"inbox/source","target":"concepts/payload","note_hash":hash,"target_hash":hash,"summary":" ","run":"run"}), "summary"),
    ].into_iter().map(|(tool, mut arguments, field)| {
        arguments["request_id"] = json!("authored-1");
        InvalidOperation { tool, arguments, field }
    }).collect()
}

#[test]
fn socket_and_mcp_payload_errors_preserve_authority_receipts_and_page_bytes() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        for slug in ["concepts/payload", "review/proposal"] {
            call(&human, &put(slug, "Original", slug), None)
                .await
                .unwrap();
        }
        let private = private_writes(dir.path());
        let page = dir.path().join("concepts/payload.md");
        let original = std::fs::read(&page).unwrap();
        for case in cases() {
            for bad_secret in [false, true] {
                let socket = raw_call(&human, &request(&human, case.wire(), bad_secret)).await;
                let mcp = mcp_call(&human, &case, bad_secret).await;
                for error in [&socket, &mcp] {
                    assert_eq!(error.request_id.as_deref(), Some("authored-1"));
                    assert!(!error.file_committed);
                    assert!(!serde_json::to_string(error).unwrap().contains("PRIVATE"));
                }
                if bad_secret {
                    for error in [&socket, &mcp] {
                        assert_eq!(error.code, crate::ErrorCode::Forbidden);
                        assert!(error.input().is_none());
                    }
                } else {
                    let kind = comb_protocol::service::input::mutation::Write::named(
                        case.tool.strip_prefix("comb_").unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        socket.input().unwrap().field(),
                        kind.pointer(case.field, false)
                    );
                    assert_eq!(
                        mcp.input().unwrap().field(),
                        format!("/arguments/{}", case.field)
                    );
                    assert!(mcp.input().unwrap().examples()[0].starts_with(case.tool));
                }
            }
        }
        assert_eq!(std::fs::read(&page).unwrap(), original);
        assert_eq!(private_writes(dir.path()), private);
        assert!(!dir.path().join(".comb/intent.json").exists());
        running.finish().await;
    });
}

#[test]
fn reviewer_authority_and_stored_corruption_keep_precedence_over_payload_checks() {
    let (dir, server, human) = setup(None);
    runtime().block_on(async {
        let running = Running::start(server);
        let ordinary = issue(
            &human,
            Grant::Client {
                name: "ordinary".into(),
                reviewer: false,
            },
        )
        .await;
        call(
            &human,
            &put("concepts/payload", "Original", "original"),
            None,
        )
        .await
        .unwrap();
        let private = private_writes(dir.path());
        let case = cases()
            .into_iter()
            .find(|case| case.tool == "comb_decide")
            .unwrap();
        for error in [
            raw_call(&ordinary, &request(&ordinary, case.wire(), false)).await,
            mcp_call(&ordinary, &case, false).await,
        ] {
            assert_eq!(error.code, crate::ErrorCode::Forbidden);
            assert!(error.input().is_none());
        }
        let path = dir.path().join("concepts/payload.md");
        std::fs::write(&path, "PRIVATE corrupt stored page").unwrap();
        for case in cases()
            .into_iter()
            .filter(|case| case.tool == "comb_put" || case.tool == "comb_append")
        {
            for error in [
                raw_call(&human, &request(&human, case.wire(), false)).await,
                mcp_call(&human, &case, false).await,
            ] {
                assert!(error.input().is_none());
                assert!(!error.file_committed);
                assert_eq!(error.request_id.as_deref(), Some("authored-1"));
                assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
            }
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "PRIVATE corrupt stored page"
        );
        assert_eq!(private_writes(dir.path()), private);
        running.finish().await;
    });
}
