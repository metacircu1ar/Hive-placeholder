use super::{ClientMethod, IncomingFrame, Model, Response, ServerMethod, outgoing};
use crate::runtime::api;
use serde_json::{Value, json};

fn exact(actual: &impl serde::Serialize, expected: &Value) {
    let actual = api::value(actual);
    assert_eq!(&actual, expected);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(expected).unwrap()
    );
}

#[test]
fn fork_retry_requires_exact_precreation_error_for_the_requested_cut() {
    for message in [
        "lastTurnId 'cut' identifies an in-progress turn",
        "lastTurnId 'cut' was not found in the source thread",
        "turn not found: cut",
        "turn cut does not have persisted rollout positions",
    ] {
        let frame = IncomingFrame::parse(json!({"id":1,"error":{"code":-32600,"message":message}}));
        assert!(frame.fork_error(Some("cut")).input_was_not_submitted());
        assert!(!frame.fork_error(Some("other")).input_was_not_submitted());
        assert!(!frame.fork_error(None).input_was_not_submitted());
        let wrong_code =
            IncomingFrame::parse(json!({"id":1,"error":{"code":-32603,"message":message}}));
        assert!(!wrong_code.fork_error(Some("cut")).input_was_not_submitted());
    }
    let ambiguous = IncomingFrame::parse(
        json!({"id":1,"error":{"code":-32600,"message":"failed after creating fork"}}),
    );
    assert!(!ambiguous.fork_error(Some("cut")).input_was_not_submitted());
}

#[test]
fn outgoing_initialize_and_metadata_match_literal_json() {
    exact(
        &outgoing::initialize(false),
        &json!({"clientInfo":{"name":"hive","title":"Hive",
        "version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true,
        "optOutNotificationMethods":["item/agentMessage/delta","item/reasoning/textDelta",
            "item/reasoning/summaryTextDelta","item/commandExecution/outputDelta"]}}),
    );
    exact(&outgoing::initialized(), &json!({"method":"initialized"}));
    exact(&outgoing::account_read(), &json!({"refreshToken":false}));
    exact(
        &outgoing::models(100),
        &json!({"includeHidden":true,"limit":100}),
    );
    for cursor in [
        Value::Null,
        json!("next"),
        json!(17),
        json!({"opaque":[null,true]}),
    ] {
        exact(
            &outgoing::model_page(100, &cursor),
            &json!({"includeHidden":true,"limit":100,"cursor":cursor}),
        );
    }
}

#[test]
fn outgoing_thread_start_resume_and_turn_match_literal_json() {
    let base = outgoing::ThreadBase {
        model: "model".into(),
        cwd: "/tmp/space here".into(),
        approval_policy: "never",
        sandbox: "read-only".into(),
        developer_instructions: "exact\ninstructions".into(),
        config: Box::new(outgoing::EffortConfig {
            mcp_servers: crate::config::McpServers::default(),
            model_reasoning_effort: "low".into(),
            model_verbosity: None,
            model_auto_compact_token_limit: None,
            tool_output_token_limit: None,
        }),
        allow_provider_model_fallback: false,
    };
    let expected = json!({"model":"model","cwd":"/tmp/space here","approvalPolicy":"never",
        "sandbox":"read-only","developerInstructions":"exact\ninstructions",
        "config":{"model_reasoning_effort":"low","mcp_servers":{}},"allowProviderModelFallback":false});
    exact(&base, &expected);
    let mut start = expected.clone();
    start["dynamicTools"] = api::tools().clone();
    exact(
        &outgoing::thread_start(Some(base.clone()), Some(api::tools().clone())),
        &start,
    );
    let mut resume = expected;
    resume["threadId"] = json!("thread");
    resume["excludeTurns"] = json!(true);
    exact(&outgoing::thread_resume(Some(base), "thread"), &resume);
    exact(
        &outgoing::turn_start("thread", "model", "low", None, "message\ntext"),
        &json!({
        "threadId":"thread","model":"model","effort":"low",
        "input":[{"type":"text","text":"message\ntext"}]}),
    );
}

#[test]
fn outgoing_cleanup_and_server_responses_match_literal_json() {
    exact(
        &outgoing::loaded_thread_page(1024, None),
        &json!({"limit":1024}),
    );
    exact(
        &outgoing::loaded_thread_page(1024, Some("next")),
        &json!({"limit":1024,"cursor":"next"}),
    );
    for request in [
        outgoing::terminal_list("thread"),
        outgoing::terminal_clean("thread"),
        outgoing::thread_unsubscribe("thread"),
    ] {
        exact(&request, &json!({"threadId":"thread"}));
    }
    exact(
        &outgoing::interrupt("thread", "turn"),
        &json!({"threadId":"thread","turnId":"turn"}),
    );
    for id in [
        Value::Null,
        json!("server-id"),
        json!(17),
        json!({"raw":[true]}),
    ] {
        for message in [
            "Provider is cleaning up.",
            "Session is not accepting tool requests.",
            "Unsupported provider request.",
        ] {
            exact(
                &outgoing::unsupported(&id, message),
                &json!({"id":id,"error":{
                "code":-32601,"message":message}}),
            );
        }
        for success in [false, true] {
            let result = json!({"raw":[null,"text"]});
            exact(
                &outgoing::tool_reply(&id, &result, success),
                &json!({"id":id,"result":{
                "contentItems":[{"type":"inputText","text":result.to_string()}],"success":success}}),
            );
        }
    }
}

#[test]
fn projection_objects_never_accept_positional_arrays() {
    let root = IncomingFrame::parse(json!([1,"turn/completed",{"threadId":"thread"}]));
    assert!(root.is_response());
    assert_eq!(root.id(), None);
    assert_eq!(root.method(), None);
    assert_eq!(root.params().thread_id(), None);
    let params = IncomingFrame::parse(json!({"method":"turn/completed",
        "params":["thread",{},null,"turn",{"id":"turn","status":"completed"}]}));
    assert_eq!(params.params().thread_id(), None);
    assert_eq!(params.params().nested_turn_id(), None);
    let error = IncomingFrame::parse(json!({"id":1,"error":[-32000,"valid rejection"]}));
    assert!(error.has_error());
    assert!(!error.valid_error());
    let raw_result = json!([{"id":"thread"},{"id":"turn"},"model","low","complete",[],
        true,{"type":"chatgpt"}]);
    let result = IncomingFrame::parse(json!({"id":1,"result":raw_result}))
        .result()
        .unwrap();
    assert_eq!(result.thread_id(), None);
    assert_eq!(result.turn_id(), None);
    assert_eq!(result.model(), None);
    assert_eq!(result.effort(), None);
    assert_eq!(result.status(), None);
    assert!(result.catalog().is_none());
    assert_eq!(result.data_is_empty(), None);
    assert!(!result.subscription_account());
    assert_eq!(result.into_raw(), raw_result);
    for value in [json!(["turn", "completed"]), json!(["thread"])] {
        let frame = IncomingFrame::parse(json!({"method":"turn/completed","params":{
            "threadId":"thread","turnId":"turn","turn":value,"thread":value,
            "item":["agentMessage"]},"result":{"thread":value,"turn":value,
            "account":["chatgpt"],"requiresOpenaiAuth":true}}));
        assert_eq!(frame.params().thread_id(), Some("thread"));
        assert_eq!(frame.params().turn_id(), Some("turn"));
        assert_eq!(frame.params().nested_turn_id(), None);
        assert_eq!(frame.params().item_kind(), None);
        let result = frame.result().unwrap();
        assert_eq!(result.thread_id(), None);
        assert_eq!(result.turn_id(), None);
        assert!(!result.subscription_account());
    }
    let malformed = Model::parse(&json!(["model",[{"reasoningEffort":"low"}]]));
    assert_eq!(malformed.name(), None);
    let model = Model::parse(&json!({"model":"model","supportedReasoningEfforts":[["low"]]}));
    assert!(!model.supports_effort("low"));
    let response = Response::parse(json!({"data":[["model",[]],
        {"model":"model","supportedReasoningEfforts":[["wrong"],{"reasoningEffort":"low"}]}]}));
    let models = response.catalog().unwrap();
    assert_eq!(models[0].name(), None);
    assert_eq!(models[1].name(), Some("model"));
    assert!(models[1].supports_effort("low"));
}

#[test]
fn response_arrays_project_only_for_catalog_consumers() {
    for raw in [
        json!({}),
        json!({"data":null}),
        json!({"data":false}),
        json!({"data":17}),
        json!({"data":"entries"}),
        json!({"data":{"model":"model"}}),
    ] {
        let response = IncomingFrame::parse(json!({"id":1,"result":raw}))
            .result()
            .unwrap();
        assert_eq!(response.data_is_empty(), None);
        assert!(response.catalog().is_none());
        assert_eq!(response.into_raw(), raw);
    }
    let entries = json!([null,false,17,"entry",["model",[]],{},
        {"model":"model","supportedReasoningEfforts":[null,["wrong"],
            {"reasoningEffort":"low"}]}]);
    for cursor in [
        Value::Null,
        json!(17),
        json!({}),
        json!([]),
        json!("next"),
        json!(false),
    ] {
        for (data, empty) in [(json!([]), true), (entries.clone(), false)] {
            let raw = json!({"data":data,"nextCursor":cursor});
            let response = IncomingFrame::parse(json!({"id":1,"result":raw}))
                .result()
                .unwrap();
            assert_eq!(response.data_is_empty(), Some(empty));
            assert_eq!(response.cursor(), &cursor);
            let catalog = response.catalog().unwrap();
            if empty {
                assert!(catalog.is_empty());
            } else {
                assert_eq!(catalog.len(), 7);
                for model in &catalog[..6] {
                    assert_eq!(model.name(), None);
                    assert!(!model.supports_effort("low"));
                }
                assert_eq!(catalog[6].name(), Some("model"));
                assert!(catalog[6].supports_effort("low"));
            }
            assert_eq!(response.into_raw(), raw);
        }
    }
}

#[test]
fn loaded_thread_inventory_is_exact_and_bounded() {
    let response = Response::parse(json!({"data":["first","second"],"nextCursor":null}));
    assert_eq!(
        response.loaded_threads().unwrap(),
        (vec!["first", "second"], None)
    );
    let response = Response::parse(json!({"data":[],"nextCursor":"next"}));
    assert_eq!(response.loaded_threads().unwrap(), (vec![], Some("next")));
    for malformed in [
        json!({}),
        json!({"data":null}),
        json!({"data":[17]}),
        json!({"data":[""]}),
        json!({"data":[],"nextCursor":17}),
        json!({"data":[],"nextCursor":""}),
    ] {
        assert!(Response::parse(malformed).loaded_threads().is_err());
    }
}

#[test]
fn method_strings_and_unknown_item_prefixes_are_exact() {
    for (method, name) in [
        (ClientMethod::Initialize, "initialize"),
        (ClientMethod::Initialized, "initialized"),
        (ClientMethod::AccountRead, "account/read"),
        (ClientMethod::ModelList, "model/list"),
        (ClientMethod::ThreadStart, "thread/start"),
        (ClientMethod::ThreadResume, "thread/resume"),
        (ClientMethod::TurnStart, "turn/start"),
        (ClientMethod::TurnInterrupt, "turn/interrupt"),
        (ClientMethod::ThreadUnsubscribe, "thread/unsubscribe"),
        (
            ClientMethod::TerminalList,
            "thread/backgroundTerminals/list",
        ),
        (
            ClientMethod::TerminalClean,
            "thread/backgroundTerminals/clean",
        ),
    ] {
        assert_eq!(method.as_str(), name);
        exact(&api::value(method), &json!(name));
    }
    for (method, name) in [
        (ServerMethod::ThreadStarted, "thread/started"),
        (ServerMethod::ThreadStatusChanged, "thread/status/changed"),
        (ServerMethod::ThreadClosed, "thread/closed"),
        (ServerMethod::TurnStarted, "turn/started"),
        (ServerMethod::TurnCompleted, "turn/completed"),
        (ServerMethod::ItemStarted, "item/started"),
        (ServerMethod::ItemCompleted, "item/completed"),
        (ServerMethod::AgentMessageDelta, "item/agentMessage/delta"),
        (ServerMethod::ReasoningTextDelta, "item/reasoning/textDelta"),
        (
            ServerMethod::ReasoningSummaryDelta,
            "item/reasoning/summaryTextDelta",
        ),
        (
            ServerMethod::CommandOutputDelta,
            "item/commandExecution/outputDelta",
        ),
        (ServerMethod::ToolCall, "item/tool/call"),
        (ServerMethod::ModelRerouted, "model/rerouted"),
        (ServerMethod::AccountUpdated, "account/updated"),
        (
            ServerMethod::RateLimitsUpdated,
            "account/rateLimits/updated",
        ),
        (ServerMethod::ConfigurationWarning, "configWarning"),
        (ServerMethod::Error, "error"),
        (ServerMethod::Other("future/method".into()), "future/method"),
        (ServerMethod::Other(String::new()), ""),
    ] {
        assert_eq!(ServerMethod::parse(name), method);
        assert_eq!(method.as_str(), name);
        exact(&api::value(&method), &json!(name));
        assert_eq!(
            serde_json::from_value::<ServerMethod>(json!(name)).unwrap(),
            method
        );
    }
    assert!(ServerMethod::parse("item/future/telemetry").is_item());
    assert!(!ServerMethod::parse("Item/future/telemetry").is_item());
}

#[test]
fn client_methods_are_closed_but_server_methods_preserve_unknown_wire_names() {
    for name in ["future/method", "", "Thread/Start"] {
        assert_eq!(api::value(ServerMethod::parse(name)), json!(name));
    }
    assert!(ClientMethod::ThreadFork.opens_thread());
    assert!(ClientMethod::TurnStart.submits_input());
    assert!(!ClientMethod::TurnSteer.opens_thread());
    assert!(ClientMethod::TerminalList.is_cleanup());
    assert!(!ClientMethod::ModelList.is_cleanup());
    assert!(
        ClientMethod::ThreadFork
            .submission_error(true, crate::runtime::provider::error::unavailable())
            .input_was_not_submitted()
    );
    assert!(
        ClientMethod::TurnStart
            .submission_error(false, crate::runtime::provider::error::unavailable())
            .input_was_not_submitted()
    );
    assert!(
        !ClientMethod::TurnStart
            .submission_error(true, crate::runtime::provider::error::unavailable())
            .input_was_not_submitted()
    );
}

#[test]
fn open_provider_decisions_preserve_unknown_raw_values_and_reject_nonstring_tags() {
    use super::{ItemKind, UnsubscribeStatus};
    for unknown in ["future", "", "AgentMessage"] {
        let raw = json!({"method":"item/completed", "params":{"item":{"type":unknown,"opaque":[1,{"secret":"unchanged"}]}}, "result":{"status":unknown}});
        let frame = IncomingFrame::parse(raw.clone());
        assert_eq!(frame.params().item_kind(), Some(ItemKind::Other));
        assert_eq!(frame.raw(), &raw);
        assert_eq!(
            serde_json::from_value::<UnsubscribeStatus>(json!(unknown)).unwrap(),
            UnsubscribeStatus::Other
        );
    }
    for malformed in [
        json!(null),
        json!(7),
        json!([]),
        json!({"agentMessage":null}),
    ] {
        assert!(serde_json::from_value::<ItemKind>(malformed.clone()).is_err());
        let raw = json!({"method":"item/completed", "params":{"item":{"type":malformed}}});
        let frame = IncomingFrame::parse(raw.clone());
        assert_eq!(frame.params().item_kind(), None);
        assert_eq!(frame.raw(), &raw);
    }
}

#[test]
fn typed_request_headers_and_parameters_match_literal_wire_values() {
    for (request, method, params) in request_cases() {
        assert_eq!(request.method().as_str(), method);
        exact(
            &outgoing::request(17, request),
            &json!({"id":17,"method":method,"params":params}),
        );
    }
    let initialize = outgoing::initialize(false);
    let params = api::value(&initialize);
    exact(
        &outgoing::request(17, initialize),
        &json!({"id":17,"method":"initialize","params":params}),
    );
}

fn request_cases() -> [(outgoing::OutgoingRequest, &'static str, Value); 13] {
    [
        (
            outgoing::account_read(),
            "account/read",
            json!({"refreshToken":false}),
        ),
        (
            outgoing::rate_limits_read(),
            "account/rateLimits/read",
            json!({}),
        ),
        (
            outgoing::models(100),
            "model/list",
            json!({"includeHidden":true,"limit":100}),
        ),
        (
            outgoing::loaded_thread_page(1024, None),
            "thread/loaded/list",
            json!({"limit":1024}),
        ),
        (
            outgoing::thread_start(None, None),
            "thread/start",
            json!({}),
        ),
        (
            outgoing::thread_resume(None, "thread"),
            "thread/resume",
            json!({"threadId":"thread","excludeTurns":true}),
        ),
        (
            outgoing::thread_fork(None, "source", "cut"),
            "thread/fork",
            json!({"threadId":"source","lastTurnId":"cut","excludeTurns":true,"ephemeral":false}),
        ),
        (
            outgoing::turn_input("thread", "text"),
            "turn/start",
            json!({"threadId":"thread","input":[{"type":"text","text":"text"}]}),
        ),
        (
            outgoing::steer("thread", "turn", "text"),
            "turn/steer",
            json!({"threadId":"thread","expectedTurnId":"turn","input":[{"type":"text","text":"text"}]}),
        ),
        (
            outgoing::interrupt("thread", "turn"),
            "turn/interrupt",
            json!({"threadId":"thread","turnId":"turn"}),
        ),
        (
            outgoing::terminal_list("thread"),
            "thread/backgroundTerminals/list",
            json!({"threadId":"thread"}),
        ),
        (
            outgoing::terminal_clean("thread"),
            "thread/backgroundTerminals/clean",
            json!({"threadId":"thread"}),
        ),
        (
            outgoing::thread_unsubscribe("thread"),
            "thread/unsubscribe",
            json!({"threadId":"thread"}),
        ),
    ]
}
