use super::{WaitUntil, Waiter, remote_response};
use crate::runtime::{self, api::Request, domain::DeliveryMode};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct Stub {
    root: tempfile::TempDir,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<Value>>>,
}

impl Stub {
    pub(super) fn new(responses: Vec<Value>) -> Self {
        Self::replies(responses.into_iter().map(Some).collect())
    }

    fn replies(responses: Vec<Option<Value>>) -> Self {
        Self::raw(
            responses
                .into_iter()
                .map(|value| value.map(|value| value.to_string()))
                .collect(),
        )
    }

    fn raw(responses: Vec<Option<String>>) -> Self {
        Self::raw_with_delay(responses, Duration::ZERO)
    }

    fn raw_with_delay(responses: Vec<Option<String>>, delay: Duration) -> Self {
        let root = tempfile::Builder::new()
            .prefix("hive-cli-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::create_dir(root.path().join("runtime")).unwrap();
        let listener = UnixListener::bind(runtime::socket_path(root.path())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            for response in responses {
                let Some(mut stream) = crate::test_support::wait_until(
                    deadline.saturating_duration_since(Instant::now()),
                    Duration::from_millis(1),
                    || {
                        if stop.load(Ordering::Relaxed) {
                            return Some(None);
                        }
                        match listener.accept() {
                            Ok((stream, _)) => Some(Some(stream)),
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                            Err(error) => panic!("accept CLI stub request: {error}"),
                        }
                    },
                ) else {
                    return requests;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                requests.push(serde_json::from_str(&line).unwrap());
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
                if let Some(response) = response {
                    writeln!(stream, "{response}").unwrap();
                }
            }
            requests
        });
        Self {
            root,
            stopping,
            worker: Some(worker),
        }
    }

    pub(super) fn root(&self) -> &Path {
        self.root.path()
    }

    pub(super) fn finish(mut self) -> Vec<Value> {
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            // Also join during assertion unwinding, before removing the socket directory.
            let _ = worker.join();
        }
    }
}

fn waiter<'a>(root: &'a Path, until: &str) -> Waiter<'a> {
    Waiter {
        root,
        target: "agent-id".into(),
        until: <WaitUntil as clap::ValueEnum>::from_str(until, false).unwrap(),
        after: 41,
        // Every decision in one polling iteration precedes the already-expired deadline.
        deadline: Instant::now(),
    }
}

fn message_snapshot(state: &str, outcome: &str) -> Value {
    json!({"not_before":null,"message_id":"message","recipient":"agent","delivery_state":state,
        "outcome":outcome,"execution_id":null,"thread_id":null,"turn_id":null,"events_after":null,
        "output":null,"output_selection":"no_text","output_partial":state!="completed",
        "queued_ahead":0,"queue_count_exact":true,"active_input":false,"recipient_state":"idle",
        "paused":false,"delivery_error":null})
}

fn assert_status_then_read(requests: &[Value]) {
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["args"],
        json!({"op":"status","target":"agent-id"})
    );
    assert_eq!(
        requests[1]["args"],
        json!({"op":"read","target":"agent-id","after":41,
        "limit":200})
    );
}

fn status_agent(outcome: &str, state: &str, expired: bool) -> Value {
    let mut fields = json!({"tool_catalog":1,"id":"agent-id","comb":null,"mcp_servers":[],"role":null,"parent_id":null,"branched_from_agent_id":null,
        "branch_hold":0,"branch_phase":null,"last_finished_turn":null,"last_attempted_turn":null,
        "auth":"main","auth_group":null,"auth_decision":null,"api_fallback":false,
        "waiting_reason":null,"quota_wait_until":null,"auth_switch_pending":null,"handoff":null,
        "turn_auth_revision":null,"execution_key":null,"model_key":null,
        "auth_probe_id":null,"auth_checked_revision":0,
        "auth_cutoff_turn":null,"auth_cutoff_message":null,"auth_generation":uuid::Uuid::nil()});
    fields.as_object_mut().unwrap().extend(
        json!({"conversation_clean":false,"profile":{"provider":"codex","method":"subscription",
        "api_key_env":null,"api_key_file":null},
        "model":"mock","effort":"low","options_revision":0,"applied_options_revision":null,"agent_options":{"options":{"model":"mock","effort":"low"},"option_sources":{}},"reserved_turn":null,
        "session_selection":null,"last_accepted_selection":null,"turn_selection":null,
        "cwd":"/tmp","provider_id":null,"provider_pid":null,"provider_spawn_before":null,
        "provider_spawn_after":null,"lifetime":"independent","sandbox":"read-only",
        "state":state,"task_outcome":outcome,"paused":false,"expired":expired,"thread_id":null,
        "execution_id":null,"turn_id":null,"control":null,"completion_pending":null,
        "result":null,"error":null,"created_at":0,"ended_at":null,
        "idle_since":null,"provider":"codex","billing":"subscription"})
        .as_object()
        .unwrap()
        .clone(),
    );
    fields
}

fn event(kind: &str, seq: u64, data: Value) -> Value {
    json!({"seq":seq,"agent_id":"agent-id","execution_id":null,"timestamp":1,
        "type":kind,"data":data})
}

fn event_page(events: Vec<Value>, cursor: u64) -> Value {
    json!({"events":events,"cursor":cursor,"history_expired":false,"pruned_through":0})
}

fn gc_page(before: u64, pending: bool, next: Option<Value>) -> Value {
    json!({"before":before,"dry_run":false,"stats":{"examined_agents":1,"eligible_agents":1,"deleted_agents":1,"blocked_agents":0,"messages":2,"attachments":0,"attachment_bytes":0,"approvals":0,"events":3,"artifacts":0,"indexed_output_bytes":0,"work_rows":6,"largest_agent_rows":6},"selected_preview":["a"],"blockers":[],"next_after":next,"cleanup_pending":pending})
}

fn input_failure() -> Value {
    json!({"error":{"code":"out_of_range","message":"Invalid page.","field":"/limit",
        "expected":"1 to 200", "examples":[r#"{"op":"agents","limit":100}"#]}})
}

fn assert_control_input(raw: Value, pointer: &str, field: &str, command: &str) {
    let input = runtime::api::InputContext::Send.error(
        hive_input::Code::InvalidValue,
        pointer,
        "a valid value",
    );
    let stub = Stub::new(vec![json!({"error":input, "request_id":"retry"})]);
    let request = runtime::api::RequestEnvelope::new(raw).request;
    let outcome = remote_response(stub.root(), "retry", request).unwrap();
    let input = outcome.input.unwrap();
    assert_eq!(input.value()["error"]["field"], field);
    assert_eq!(input.value()["request_id"], "retry");
    let rendered = serde_json::from_value::<hive_input::InputError>(input.value()["error"].clone())
        .unwrap()
        .render(hive_input::Product::Hive)
        .unwrap();
    assert_eq!(rendered.exit_code, 2);
    assert_eq!(
        input.value()["error"]["examples"],
        serde_json::to_value(super::catalog::examples(command).unwrap().as_slice()).unwrap()
    );
    stub.finish();
}

mod cases_behaviour_1;
mod cases_recovery_1;
mod cases_requests_1;
mod cases_transport_1;
mod cases_transport_2;
