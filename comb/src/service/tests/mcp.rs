use super::*;

#[test]
fn mcp_frames_share_service_and_maximum_escaped_page_remains_readable() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let (stop,stopped)=tokio::sync::oneshot::channel();let running=tokio::spawn(server.run(async{let _=stopped.await;}));
        let credential=issue(&human,Grant::Client{name:"mcp".into(),reviewer:false}).await;
        let body=format!("Visible{}tail","\u{1}".repeat(crate::page::PAGE_BYTES-4096));
        call(&human,&put("concepts/escaped",&body,"large"),None).await.unwrap();
        let direct=call(&human,&Operation::Get{slug:"concepts/escaped".parse().unwrap()},None).await.unwrap();
        assert!(framing::encode(&direct,framing::RESPONSE_BYTES).unwrap().len()>framing::REQUEST_BYTES);
        let (client_stream,server_stream)=UnixStream::pair().unwrap();
        let (input,output)=server_stream.into_split();
        let proxy=tokio::spawn(super::super::mcp::run(credential.clone(),BufReader::new(input),output));
        let mut wire=BufReader::new(client_stream);
        for frame in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"concepts/escaped"}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"comb_put","arguments":{"slug":"concepts/forged","markdown":"---\ntitle: Forged\n---\nText","actor":{"kind":"human"}}}}),
            json!({"jsonrpc":"2.0","id":5,"method":"unknown"}),
        ] {framing::write(wire.get_mut(),&frame,framing::REQUEST_BYTES).await.unwrap();}
        let mut replies=Vec::new();for _ in 0..5 {replies.push(serde_json::from_slice::<Value>(&framing::read(&mut wire,framing::MCP_BYTES).await.unwrap().unwrap()).unwrap());}
        assert_eq!(replies[0]["result"]["protocolVersion"],"2025-11-25");
        assert!(!replies[1]["result"]["tools"].as_array().unwrap().iter().any(|t|t["name"]=="comb_promote"));
        assert_eq!(replies[2]["result"]["isError"],false);
        let doc:Value=serde_json::from_str(replies[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(doc["page"]["compiled"],body);
        assert_eq!(replies[3]["result"]["isError"],true);assert_eq!(replies[4]["error"]["code"],-32601);
        drop(wire);proxy.await.unwrap().unwrap();
        stop.send(()).unwrap();running.await.unwrap().unwrap();
    });
}

#[test]
fn typed_tool_arguments_reject_authority_fields_and_keep_request_identity() {
    for name in [
        "comb_put",
        "comb_append",
        "comb_capture",
        "comb_promote",
        "comb_get",
        "comb_search",
        "comb_list",
    ] {
        assert!(
            super::super::mcp::operation(name, json!({"actor":"human","reviewer":true}), None)
                .is_err()
        );
    }
    let op = super::super::mcp::operation(
        "comb_capture",
        json!({"text":"note"}),
        Some("provider-call"),
    )
    .unwrap();
    assert_eq!(op.request_id(), Some("provider-call"));
    let op = super::super::mcp::operation(
        "comb_capture",
        json!({"text":"note","request_id":"explicit"}),
        Some("provider-call"),
    )
    .unwrap();
    assert_eq!(op.request_id(), Some("explicit"));
    assert!(super::super::mcp::operation("comb_import", json!({}), None).is_err());
}

#[test]
fn deletion_has_no_agent_tool_or_parser_alias() {
    for name in ["comb_rm", "comb_remove"] {
        for reviewer in [false, true] {
            assert!(
                !super::super::mcp::tools(reviewer)
                    .iter()
                    .any(|tool| tool.name == name)
            );
        }
        let error = super::super::mcp::operation(
            name,
            json!({
                "slug":"concepts/remove", "expected_hash":"ab".repeat(32), "request_id":"remove"
            }),
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.input().unwrap().field(), "/name");
        assert_eq!(
            error.input().unwrap().code(),
            hive_input::Code::InvalidValue
        );
    }
}

async fn mcp_put_reply(connection: Connection, markdown: &str, request_id: &str) -> Value {
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let (input, output) = server_stream.into_split();
    let proxy = tokio::spawn(super::super::mcp::run(
        connection,
        BufReader::new(input),
        output,
    ));
    let mut wire = BufReader::new(client_stream);
    for frame in [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "clientInfo": {}, "capabilities": {}
            }
        }),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {
                "name": "comb_put",
                "arguments": {
                    "slug": "concepts/diagnostics", "markdown": markdown,
                    "request_id": request_id
                }
            }
        }),
    ] {
        framing::write(wire.get_mut(), &frame, framing::REQUEST_BYTES)
            .await
            .unwrap();
    }
    let mut replies = Vec::new();
    for _ in 0..2 {
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            framing::read(&mut wire, framing::MCP_BYTES),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        replies.push(serde_json::from_slice::<Value>(&bytes).unwrap());
    }
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
    drop(wire);
    proxy.await.unwrap().unwrap();
    replies.pop().unwrap()
}

#[test]
fn socket_and_mcp_frontmatter_errors_omit_private_causes() {
    let (_dir, server, human) = setup(None);
    let marker = "private-frontmatter-marker";
    let markdown = format!("---\ntitle: Diagnostics\nexternal: {marker}\n---\nText\n");
    let local = crate::Page::parse(&markdown).unwrap_err();
    assert!(
        std::error::Error::source(&local)
            .unwrap()
            .to_string()
            .contains(marker)
    );
    runtime().block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let running = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        let op = Operation::Put {
            slug: "concepts/diagnostics".parse().unwrap(),
            markdown: markdown.clone(),
            expected_hash: None,
            request_id: "frontmatter-rejection".parse().unwrap(),
            run: None,
        };
        let error = call(&human, &op, None).await.unwrap_err();
        assert_eq!(error.input().unwrap().field(), "/operation/markdown");
        assert_eq!(error.request_id.as_deref(), Some("frontmatter-rejection"));
        assert!(!error.file_committed);
        assert!(std::error::Error::source(&error).is_none());
        let socket_error = serde_json::to_value(error).unwrap();
        assert_eq!(socket_error.as_object().unwrap().len(), 7);
        assert!(!socket_error.to_string().contains(marker));
        let reply = mcp_put_reply(human, &markdown, "frontmatter-rejection").await;
        assert_eq!(reply["id"], 2);
        assert_eq!(reply["result"]["isError"], true);
        assert!(!reply.to_string().contains(marker));
        let body: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["error"]["field"], "/arguments/markdown");
        assert_eq!(body["error"]["request_id"], socket_error["request_id"]);
        assert_eq!(body["error"]["file_committed"], false);
        assert!(
            body["error"]["examples"][0]
                .as_str()
                .unwrap()
                .starts_with("comb_put ")
        );
        stop.send(()).unwrap();
        running.await.unwrap().unwrap();
    });
}

#[test]
fn oversized_mcp_frames_return_a_bounded_protocol_error_and_close() {
    let (_dir, _server, human) = setup(None);
    runtime().block_on(async {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let (input, output) = server_stream.into_split();
        let proxy = tokio::spawn(super::super::mcp::run(human, BufReader::new(input), output));
        let (reader, mut writer) = client_stream.into_split();
        let sending = tokio::spawn(async move {
            let _ = writer
                .write_all(&vec![b' '; framing::REQUEST_BYTES + 1])
                .await;
        });
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            framing::read(&mut BufReader::new(reader), 1024),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["error"]["code"],
            -32600
        );
        assert_eq!(
            proxy.await.unwrap().unwrap_err().code,
            crate::ErrorCode::SizeLimit
        );
        sending.await.unwrap();
    });
}

#[test]
fn mcp_ping_and_tools_observe_the_initialization_boundary() {
    let (_dir, server, human) = setup(None);
    runtime().block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let running = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        let initialize = |id| {
            json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","clientInfo":{},"capabilities":{}
            }})
        };
        let frames = [
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":0,"method":"ping"}),
            initialize(1),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
            initialize(4),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            initialize(6),
            json!({"jsonrpc":"2.0","id":7,"method":"ping"}),
        ];
        let input = frames.map(|frame| format!("{frame}\n")).concat();
        let mut output = Vec::new();
        super::super::mcp::run(human, input.as_bytes(), &mut output)
            .await
            .unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 8);
        for id in [0, 2, 4] {
            assert_eq!(replies[id]["error"]["code"], -32000);
        }
        assert_eq!(replies[1]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(replies[3]["result"], json!({}));
        assert!(replies[5]["result"]["tools"].as_array().unwrap().len() > 1);
        assert_eq!(replies[6]["error"]["code"], -32601);
        assert_eq!(replies[7]["result"], json!({}));
        stop.send(()).unwrap();
        running.await.unwrap().unwrap();
    });
}
