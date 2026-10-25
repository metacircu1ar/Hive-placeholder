use super::corpus_support::{events, execute};
use crate::runtime::{
    provider::WorkerTransport,
    test_support::{RuntimeFixture, agent},
};
use serde_json::json;

#[tokio::test]
async fn full_access_codex_start_and_resume_use_native_mode_and_never_approvals() {
    for resumed in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut current = agent();
        current.sandbox = crate::config::Sandbox::FullAccess;
        if !resumed {
            current.thread_id = None;
        }
        fixture
            .hub
            .with({
                let current = current.clone();
                move |core| core.store().save(&current)
            })
            .await
            .unwrap();
        let mut transport = WorkerTransport::new(
            &fixture,
            json!({"thread":{"id":"thread"},"model":"gpt-5.6-luna","reasoningEffort":"low","sandbox":{"type":"danger-full-access"}}),
        );
        execute(&fixture, current, &mut transport).await.unwrap();
        {
            let sent = transport.sent.lock().unwrap();
            let request = sent
                .iter()
                .find(|r| {
                    r["method"]
                        == if resumed {
                            "thread/resume"
                        } else {
                            "thread/start"
                        }
                })
                .unwrap();
            assert_eq!(request["params"]["sandbox"], "danger-full-access");
            assert_eq!(request["params"]["approvalPolicy"], "never");
        }
        transport.finish().await;
        fixture.finish();
    }
}
#[tokio::test]
async fn thread_ack_validation_order_keeps_new_binding() {
    for (result, expected, bound) in [
        (json!(null), "invalid_request", false),
        (json!({"thread":{"id":17}}), "invalid_request", false),
        (
            json!({"thread":{"id":"new-thread"}}),
            "model_mismatch",
            true,
        ),
        (
            json!({"thread":{"id":"new-thread"},"model":"gpt-5.6-luna","reasoningEffort":null}),
            "effort_mismatch",
            true,
        ),
    ] {
        let fixture = RuntimeFixture::new();
        fixture
            .hub
            .with(|core| {
                let mut agent = core.store().agent("agent")?;
                agent.thread_id = None;
                core.store().save(&agent)
            })
            .await
            .unwrap();
        let mut transport = WorkerTransport::new(&fixture, result);
        let mut agent = agent();
        agent.thread_id = None;
        let error = execute(&fixture, agent, &mut transport).await.unwrap_err();
        assert_eq!(error.code(), expected);
        let saved = fixture
            .hub
            .with(|core| core.store().agent("agent"))
            .await
            .unwrap();
        assert_eq!(saved.thread_id.as_deref(), bound.then_some("new-thread"));
        assert_eq!(
            events(&fixture).await["events"].as_array().unwrap().len(),
            usize::from(bound)
        );
        transport.finish().await;
        fixture.finish();
    }
}

#[tokio::test]
async fn valid_session_ack_keeps_sandbox_and_resume_precedence() {
    for resumed in [false, true] {
        let fixture = RuntimeFixture::new();
        let mut agent = agent();
        if !resumed {
            agent.thread_id = None;
            fixture
                .hub
                .with(|core| {
                    let mut agent = core.store().agent("agent")?;
                    agent.thread_id = None;
                    core.store().save(&agent)
                })
                .await
                .unwrap();
        }
        let sandbox = json!({"type":"futureSandbox","opaque":[null,17,{"kept":true}]});
        let result = json!({"thread":{"id":"thread","extra":false},"model":"gpt-5.6-luna",
            "reasoningEffort":"low","sandbox":sandbox,"extra":[1,null]});
        let mut transport = WorkerTransport::new(&fixture, result);
        assert!(!execute(&fixture, agent, &mut transport).await.unwrap());
        let stored = events(&fixture).await;
        let event = stored["events"].as_array().unwrap().last().unwrap();
        assert_eq!(event["type"], "session_ready");
        assert_eq!(
            event["data"],
            json!({"thread_id":"thread","model":"gpt-5.6-luna",
            "effort":"low","options_revision":0,"selection_source":"reserved","sandbox":sandbox,"resumed":resumed})
        );
        assert_eq!(transport.session.thread.as_deref(), Some("thread"));
        transport.finish().await;
        fixture.finish();
    }
    for (model, effort, expected) in [
        (json!(false), json!(null), "model_mismatch"),
        (json!("gpt-5.6-luna"), json!(false), "effort_mismatch"),
        (json!("gpt-5.6-luna"), json!("low"), "session_mismatch"),
    ] {
        let fixture = RuntimeFixture::new();
        let mut transport = WorkerTransport::new(
            &fixture,
            json!({"thread":{"id":"wrong-thread"},
            "model":model,"reasoningEffort":effort}),
        );
        assert_eq!(
            execute(&fixture, agent(), &mut transport)
                .await
                .unwrap_err()
                .code(),
            expected
        );
        let saved = fixture
            .hub
            .with(|core| core.store().agent("agent"))
            .await
            .unwrap();
        assert_eq!(saved.thread_id.as_deref(), Some("thread"));
        assert_eq!(events(&fixture).await["events"], json!([]));
        transport.finish().await;
        fixture.finish();
    }
}
