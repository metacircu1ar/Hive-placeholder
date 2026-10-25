//! `claude.skills`: launch arguments, the `Skill` tool policy and forked skills.
use super::*;

const MODEL: &str = "claude-sonnet-5";

fn skills(config: &mut SessionConfig, enabled: bool) {
    Arc::make_mut(&mut config.options).values = [("claude.skills".into(), json!(enabled))]
        .into_iter()
        .collect();
}

fn launch(configure: impl FnOnce(&mut SessionConfig)) -> (bool, String) {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::for_test(
        Uuid::new_v4().to_string(),
        dir.path().canonicalize().unwrap(),
    );
    configure(&mut config);
    let mut command = std::process::Command::new("claude");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    let tools = args[args.iter().position(|a| *a == "--tools").unwrap() + 1].to_owned();
    (args.contains(&"--disable-slash-commands"), tools)
}

#[test]
fn the_option_enables_slash_commands_and_the_skill_tool_except_for_the_canary() {
    assert_eq!(launch(|_| {}), (true, String::from("Bash,Read,Glob,Grep")));
    assert_eq!(
        launch(|config| skills(config, false)),
        (true, String::from("Bash,Read,Glob,Grep"))
    );
    assert_eq!(
        launch(|config| skills(config, true)),
        (false, String::from("Bash,Read,Glob,Grep,Skill"))
    );
    assert_eq!(
        launch(|config| {
            skills(config, true);
            config.sandbox = Sandbox::WorkspaceWrite;
        }),
        (
            false,
            String::from("Bash,Read,Glob,Grep,Edit,Write,NotebookEdit,Skill")
        )
    );
    assert_eq!(
        launch(|config| {
            skills(config, true);
            config.steering = Some(SteeringPermit::Canary);
        }),
        (true, String::from("Read"))
    );
}

fn fixture(enabled: bool, sandbox: Sandbox) -> Fixture {
    Fixture::with_extensions(None, Uuid::new_v4().to_string(), |config| {
        skills(config, enabled);
        config.sandbox = sandbox;
    })
}

impl Fixture {
    fn frame(&mut self, frame: Value) -> crate::error::Result<Vec<Value>> {
        self.codec.receive(frame)
    }
    fn pre(&mut self, tool: &str, id: &str, input: Value) -> Value {
        let reply = self.frame(json!({"type":"control_request","request_id":format!("pre-{id}"),"request":{
            "subtype":"hook_callback","callback_id":"hive-guard","input":{
            "hook_event_name":"PreToolUse","session_id":self.id,"tool_name":tool,"tool_use_id":id,"tool_input":input}}}))
            .unwrap();
        reply[0]["response"]["response"]["hookSpecificOutput"]["permissionDecision"].clone()
    }
    fn post(&mut self, tool: &str, id: &str) -> Value {
        self.frame(json!({"type":"control_request","request_id":format!("post-{id}"),"request":{
            "subtype":"hook_callback","callback_id":"hive-finished","input":{
            "hook_event_name":"PostToolUse","session_id":self.id,"tool_use_id":id,"tool_name":tool}}}))
            .unwrap()
            .remove(0)
    }
    fn tool_use(&mut self, parent: Option<&str>, id: &str, name: &str, input: Value) {
        self.frame(
            json!({"type":"assistant","session_id":self.id,"parent_tool_use_id":parent,"message":{
            "model":MODEL,"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}}),
        )
        .unwrap();
    }
    fn tool_result(&mut self, parent: Option<&str>, id: &str) {
        self.frame(
            json!({"type":"user","session_id":self.id,"parent_tool_use_id":parent,"message":{
            "content":[{"type":"tool_result","tool_use_id":id,"content":"done"}]}}),
        )
        .unwrap();
    }
    fn task(
        &mut self,
        subtype: &str,
        task: &str,
        tool: &str,
        backgrounded: bool,
    ) -> crate::error::Result<Vec<Value>> {
        self.frame(json!({"type":"system","subtype":subtype,"session_id":self.id,"task_id":task,
            "tool_use_id":tool,"is_backgrounded":backgrounded,"task_type":"local_agent","status":"completed"}))
    }
    /// A top-level `Skill` call that the hook allowed.
    fn skill(&mut self) {
        self.tool_use(None, "skill-1", "Skill", json!({"skill":"code-review"}));
        assert_eq!(
            self.pre("Skill", "skill-1", json!({"skill":"code-review"})),
            "allow"
        );
    }
    /// The emitted frames' methods, and whether any frame carries `text`.
    fn methods(&mut self, text: &str) -> (Vec<String>, bool) {
        let frames: Vec<Value> = std::iter::from_fn(|| self.codec.pop()).collect();
        (
            frames
                .iter()
                .map(|frame| frame["method"].as_str().unwrap_or("reply").to_owned())
                .collect(),
            frames.iter().any(|frame| frame.to_string().contains(text)),
        )
    }
}

#[test]
fn skill_calls_are_allowed_only_when_enabled_on_both_permission_paths() {
    for enabled in [false, true] {
        let mut f = fixture(enabled, Sandbox::ReadOnly);
        f.turn();
        let expected = if enabled { "allow" } else { "deny" };
        assert_eq!(
            f.pre("Skill", "skill-1", json!({"skill":"code-review"})),
            expected
        );
        let reply = f
            .frame(
                json!({"type":"control_request","request_id":"permission","request":{
            "subtype":"can_use_tool","tool_name":"Skill","input":{"skill":"code-review"}}}),
            )
            .unwrap();
        assert_eq!(reply[0]["response"]["response"]["behavior"], expected);
    }
}

/// Inside a read-only agent's fork: only read-only git, no writes, no second skill.
fn denied_inside_the_fork(f: &mut Fixture) {
    f.tool_use(
        Some("skill-1"),
        "sub-2",
        "Bash",
        json!({"command":"git diff; touch x"}),
    );
    assert_eq!(
        f.pre("Bash", "sub-2", json!({"command":"git diff; touch x"})),
        "deny"
    );
    f.tool_use(
        Some("skill-1"),
        "sub-3",
        "Write",
        json!({"file_path":"/tmp/x"}),
    );
    assert_eq!(
        f.pre("Write", "sub-3", json!({"file_path":"/tmp/x"})),
        "deny"
    );
    // A forked skill cannot start another skill.
    f.tool_use(
        Some("skill-1"),
        "sub-4",
        "Skill",
        json!({"skill":"simplify"}),
    );
    assert_eq!(f.pre("Skill", "sub-4", json!({"skill":"simplify"})), "deny");
}

#[test]
fn a_forked_skill_completes_its_turn_under_the_agents_tool_policy() {
    let mut f = fixture(true, Sandbox::ReadOnly);
    let turn = f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    // The fork's prompt, a streamed delta and a heartbeat are not the agent's output.
    f.frame(
        json!({"type":"user","session_id":f.id,"parent_tool_use_id":"skill-1","isSynthetic":true,
        "message":{"content":[{"type":"text","text":"You are reviewing a pull request."}]}}),
    )
    .unwrap();
    f.frame(
        json!({"type":"stream_event","session_id":f.id,"parent_tool_use_id":"skill-1",
        "event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}}}),
    )
    .unwrap();
    f.frame(
        json!({"type":"tool_progress","session_id":f.id,"parent_tool_use_id":"skill-1",
        "tool_use_id":"skill-1-heartbeat-0","tool_name":"Skill","heartbeat":true}),
    )
    .unwrap();
    // A replay-marked prompt is the fork's, never this turn's acknowledgement.
    f.frame(
        json!({"type":"user","session_id":f.id,"parent_tool_use_id":"skill-1","isReplay":true,
        "uuid":Uuid::new_v4().to_string(),"message":{"content":"Review the diff."}}),
    )
    .unwrap();
    // The fork may run on another model; only its tool calls count.
    f.frame(
        json!({"type":"assistant","session_id":f.id,"parent_tool_use_id":"skill-1","message":{
        "model":"claude-opus-9","content":[{"type":"tool_use","id":"sub-1","name":"Read",
        "input":{"file_path":"/tmp/fixed"}}]}}),
    )
    .unwrap();
    assert_eq!(
        f.pre("Read", "sub-1", json!({"file_path":"/tmp/fixed"})),
        "allow"
    );
    denied_inside_the_fork(&mut f);
    f.frame(
        json!({"type":"assistant","session_id":f.id,"parent_tool_use_id":"skill-1","message":{
        "model":MODEL,"content":[{"type":"text","text":"subagent findings"}]}}),
    )
    .unwrap();
    f.tool_result(Some("skill-1"), "sub-1");
    let post = f.post("Read", "sub-1");
    assert!(
        post["response"]["response"]
            .get("hookSpecificOutput")
            .is_none(),
        "{post}"
    );
    f.task("task_notification", "task-1", "skill-1", false)
        .unwrap();
    // The fork lasts until its Skill call settles, so a trailing frame is fine.
    f.frame(
        json!({"type":"stream_event","session_id":f.id,"parent_tool_use_id":"skill-1",
        "event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"y"}}}),
    )
    .unwrap();
    f.tool_result(None, "skill-1");
    f.post("Skill", "skill-1");
    f.assistant(
        None,
        MODEL,
        json!([{"type":"text","text":"The review found a bug."}]),
    );
    f.frame(
        json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,
        "terminal_reason":"completed","user_message_uuid":turn}),
    )
    .unwrap();
    let (methods, subagent_text) = f.methods("subagent findings");
    // The Skill call and the fork's Read, Bash, Write and Skill calls are
    // shown; the subagent's text is not.
    assert!(!subagent_text, "{methods:?}");
    assert_eq!(
        methods.iter().filter(|m| *m == "item/started").count(),
        5,
        "{methods:?}"
    );
    assert_eq!(
        methods.last().map(String::as_str),
        Some("turn/completed"),
        "{methods:?}"
    );
    assert!(f.codec.quiescent());
}

#[test]
fn fork_frames_are_refused_unless_they_belong_to_an_open_allowed_skill_fork() {
    // The option is off: the Skill call is denied, so no fork can start.
    let mut f = fixture(false, Sandbox::ReadOnly);
    f.turn();
    f.tool_use(None, "skill-1", "Skill", json!({"skill":"code-review"}));
    assert_eq!(
        f.pre("Skill", "skill-1", json!({"skill":"code-review"})),
        "deny"
    );
    assert!(f.task("task_started", "task-1", "skill-1", false).is_err());
    // A task under another tool, or in the background.
    let mut f = fixture(true, Sandbox::ReadOnly);
    f.turn();
    f.tool_use(None, "read-1", "Read", json!({"file_path":"/tmp/fixed"}));
    assert_eq!(
        f.pre("Read", "read-1", json!({"file_path":"/tmp/fixed"})),
        "allow"
    );
    assert!(f.task("task_started", "task-1", "read-1", false).is_err());
    let mut f = fixture(true, Sandbox::ReadOnly);
    f.turn();
    f.skill();
    assert!(f.task("task_started", "task-1", "skill-1", true).is_err());
    // A second fork, a foreign notification and a result while the fork is open.
    let mut f = fixture(true, Sandbox::ReadOnly);
    f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    assert!(f.task("task_started", "task-2", "skill-1", false).is_err());
    let mut f = fixture(true, Sandbox::ReadOnly);
    f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    assert!(
        f.task("task_notification", "task-9", "skill-1", false)
            .is_err()
    );
    let mut f = fixture(true, Sandbox::ReadOnly);
    let turn = f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    assert!(!f.codec.quiescent());
    let result = f.frame(
        json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,
        "terminal_reason":"completed","user_message_uuid":turn}),
    );
    assert!(result.is_err());
    // A notification alone does not settle the Skill call.
    let mut f = fixture(true, Sandbox::ReadOnly);
    let turn = f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    f.task("task_notification", "task-1", "skill-1", false)
        .unwrap();
    let result = f.frame(
        json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,
        "terminal_reason":"completed","user_message_uuid":turn}),
    );
    assert!(result.is_err());
    // Frames under a parent with no open fork, and progress without a fork.
    let mut f = fixture(true, Sandbox::ReadOnly);
    f.turn();
    f.skill();
    assert!(
        f.frame(
            json!({"type":"assistant","session_id":f.id,"parent_tool_use_id":"skill-1","message":{
        "model":MODEL,"content":[{"type":"text","text":"early"}]}})
        )
        .is_err()
    );
    let mut f = fixture(true, Sandbox::ReadOnly);
    f.turn();
    assert!(f.task("task_progress", "task-1", "skill-1", false).is_err());
}

#[test]
fn a_fork_settled_by_its_finished_hook_ignores_its_late_task_frames() {
    let mut f = fixture(true, Sandbox::ReadOnly);
    let turn = f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    // The hook settles the Skill call first, as on an aborted skill; the late
    // notification and result for the settled fork are accepted.
    f.post("Skill", "skill-1");
    f.task("task_notification", "task-1", "skill-1", false)
        .unwrap();
    f.task("task_progress", "task-1", "skill-1", false).unwrap();
    assert!(
        f.task("task_notification", "task-2", "skill-1", false)
            .is_err()
    );
    f.tool_result(None, "skill-1");
    f.frame(
        json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,
        "terminal_reason":"completed","user_message_uuid":turn}),
    )
    .unwrap();
    assert_eq!(
        f.methods("").0.last().map(String::as_str),
        Some("turn/completed")
    );
    assert!(f.codec.quiescent());
}

#[test]
fn steering_guidance_waits_for_the_skill_call_not_the_forks_tools() {
    let mut f = Fixture::with_extensions(
        Some(SteeringPermit::Verified(ReceiptMode::NativeEcho)),
        Uuid::new_v4().to_string(),
        |config| skills(config, true),
    );
    let turn = f.turn();
    f.skill();
    f.task("task_started", "task-1", "skill-1", false).unwrap();
    assert!(
        f.codec
            .send(outgoing::request(
                10,
                outgoing::steer(&f.id, &turn, "focus on tests")
            ))
            .unwrap()
            .is_empty()
    );
    f.tool_use(
        Some("skill-1"),
        "sub-1",
        "Read",
        json!({"file_path":"/tmp/fixed"}),
    );
    assert_eq!(
        f.pre("Read", "sub-1", json!({"file_path":"/tmp/fixed"})),
        "allow"
    );
    f.tool_result(Some("skill-1"), "sub-1");
    let nested = f.post("Read", "sub-1");
    assert!(
        nested["response"]["response"]
            .get("hookSpecificOutput")
            .is_none(),
        "{nested}"
    );
    f.task("task_notification", "task-1", "skill-1", false)
        .unwrap();
    f.tool_result(None, "skill-1");
    let own = f.post("Skill", "skill-1");
    assert_eq!(
        own["response"]["response"]["hookSpecificOutput"]["additionalContext"], "focus on tests",
        "{own}"
    );
}
