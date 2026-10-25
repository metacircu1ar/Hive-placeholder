use super::*;
use serde_json::json;

fn agent(id: &str, parent: Option<&str>, state: &str) -> Agent {
    Agent {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        role: Some(format!("role-{id}")),
        name: None,
        state: state.to_owned(),
        sandbox: String::from("workspace-write"),
        outcome: String::from("open"),
        waiting: None,
        approval: None,
        ended: false,
    }
}

fn ids(sidebar: &Sidebar) -> Vec<(String, usize)> {
    sidebar
        .items()
        .into_iter()
        .map(|item| (item.id, item.depth))
        .collect()
}

fn none(_: &str) -> Option<String> {
    None
}

fn press(sidebar: &mut Sidebar, code: KeyCode) -> Key {
    sidebar.key(&KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn rows_parse_tree_replies_with_every_badge_input() {
    let reply = json!({"agents": [
        {"id": "a", "parent_id": null, "role": "lead", "state": "running", "task_outcome": "open",
         "waiting_reason": "quota", "approval_id": "q1", "ended_at": null},
        {"id": "b", "parent_id": "a", "role": null, "state": "idle", "task_outcome": "completed",
         "waiting_reason": null, "approval_id": null, "ended_at": 5},
        {"parent_id": "a"},
    ], "truncated": false});
    let rows = rows(&reply);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].badge(), rows[0].waiting.as_deref(), rows[0].label()),
        (Badge::NeedsYou, Some("quota"), "lead")
    );
    assert_eq!(
        (rows[1].badge(), rows[1].ended, rows[1].label()),
        (Badge::Done, true, "agent")
    );
}

#[test]
fn badges_follow_approval_quota_state_and_outcome() {
    let mut row = agent("a", None, "running");
    assert_eq!(row.badge(), Badge::Working);
    row.waiting = Some(String::from("quota"));
    assert_eq!(row.badge(), Badge::Quota);
    row.approval = Some(String::from("q"));
    assert_eq!(row.badge(), Badge::NeedsYou);
    let mut idle = agent("b", None, "stopped");
    assert_eq!(idle.badge(), Badge::Idle);
    idle.state = String::from("failed");
    assert_eq!(idle.badge(), Badge::Failed);
    let mut ended = agent("c", None, "idle");
    ended.ended = true;
    for (outcome, badge) in [
        ("completed", Badge::Done),
        ("cancelled", Badge::Cancelled),
        ("open", Badge::Ended),
    ] {
        ended.outcome = outcome.to_owned();
        assert_eq!(ended.badge(), badge, "{outcome}");
    }
    // An ended agent's stale approval does not ask for attention.
    ended.approval = Some(String::from("q"));
    assert_ne!(ended.badge(), Badge::NeedsYou);
}

#[test]
fn rows_group_under_the_nearest_shown_ancestor() {
    let mut sidebar = Sidebar::new(false);
    sidebar.update(
        vec![
            agent("root", None, "idle"),
            agent("mid", Some("root"), "running"),
            agent("leaf", Some("mid"), "running"),
            agent("other", None, "idle"),
        ],
        false,
        none,
    );
    assert_eq!(
        ids(&sidebar),
        [
            (String::from("root"), 0),
            (String::from("mid"), 1),
            (String::from("leaf"), 2),
            (String::from("other"), 0)
        ]
    );
    // "mid" ended and left the live tree: "leaf" moves up under "root".
    sidebar.update(
        vec![
            agent("root", None, "idle"),
            agent("leaf", Some("mid"), "running"),
            agent("other", None, "idle"),
        ],
        false,
        none,
    );
    assert_eq!(
        ids(&sidebar)[..2],
        [(String::from("root"), 0), (String::from("leaf"), 1)]
    );
    // Ended rows (from a --root tree) are hidden without --keep-ended.
    let mut ended = agent("done", Some("root"), "idle");
    ended.ended = true;
    sidebar.update(vec![agent("root", None, "idle"), ended], false, none);
    assert_eq!(ids(&sidebar), [(String::from("root"), 0)]);
}

#[test]
fn collapsed_parents_roll_up_counts_and_the_strongest_badge() {
    let mut sidebar = Sidebar::new(false);
    let mut asking = agent("c2", Some("c1"), "idle");
    asking.approval = Some(String::from("q"));
    sidebar.update(
        vec![
            agent("p", None, "idle"),
            agent("c1", Some("p"), "running"),
            asking,
            agent("c3", Some("p"), "idle"),
        ],
        false,
        none,
    );
    // Left on a parent collapses it.
    assert_eq!(press(&mut sidebar, KeyCode::Left), Key::None);
    let items = sidebar.items();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].hidden, items[0].badge),
        (Some(3), Badge::NeedsYou)
    );
    assert_eq!(press(&mut sidebar, KeyCode::Right), Key::None);
    assert_eq!(sidebar.items().len(), 4);
    // Idle and ended descendants never roll up; working does.
    let mut quiet = Sidebar::new(false);
    quiet.update(
        vec![agent("p", None, "idle"), agent("c", Some("p"), "running")],
        false,
        none,
    );
    press(&mut quiet, KeyCode::Left);
    assert_eq!(quiet.items()[0].badge, Badge::Working);
}

#[test]
fn keep_ended_retains_agents_that_leave_the_tree_with_their_outcome() {
    let mut sidebar = Sidebar::new(true);
    sidebar.update(
        vec![agent("a", None, "running"), agent("b", None, "running")],
        false,
        none,
    );
    sidebar.update(vec![agent("a", None, "running")], false, |id| {
        (id == "b").then(|| String::from("completed"))
    });
    let b = sidebar.agent("b").unwrap();
    assert!(b.ended && b.badge() == Badge::Done);
    // A truncated tree may omit live agents: they are not marked ended.
    sidebar.update(vec![agent("b2", None, "running")], true, none);
    assert!(sidebar.agent("a").is_none());
    assert!(sidebar.agent("b").is_some_and(|b| b.ended));
    assert!(sidebar.truncated);
    // Without --keep-ended they simply disappear.
    let mut plain = Sidebar::new(false);
    plain.update(vec![agent("a", None, "running")], false, none);
    plain.update(Vec::new(), false, none);
    assert!(plain.agents().is_empty());
}

#[test]
fn retained_ended_agents_are_bounded() {
    let mut sidebar = Sidebar::new(true);
    let many: Vec<Agent> = (0..RETAINED_ENDED + 5)
        .map(|index| agent(&format!("a{index}"), None, "running"))
        .collect();
    sidebar.update(many, false, none);
    sidebar.update(vec![agent("late", None, "running")], false, none);
    assert_eq!(sidebar.agents().len(), RETAINED_ENDED + 1);
    assert_eq!(sidebar.agents()[1].id, "a5");
    // A later batch keeps the newest ended agent and drops the oldest.
    sidebar.update(Vec::new(), false, none);
    let agents = sidebar.agents();
    assert_eq!(agents.len(), RETAINED_ENDED);
    assert_eq!(
        (
            agents[0].id.as_str(),
            agents[RETAINED_ENDED - 1].id.as_str()
        ),
        ("a6", "late")
    );
}

#[test]
fn typing_filters_fuzzily_and_enter_opens_the_selection() {
    let mut sidebar = Sidebar::new(false);
    let mut reviewer = agent("1234567890", None, "running");
    reviewer.role = Some(String::from("reviewer"));
    let mut worker = agent("abcdefabcd", None, "idle");
    worker.role = Some(String::from("worker"));
    sidebar.update(vec![reviewer, worker], false, none);
    for character in "rvw".chars() {
        press(&mut sidebar, KeyCode::Char(character));
    }
    assert_eq!(ids(&sidebar), [(String::from("1234567890"), 0)]);
    assert_eq!(
        press(&mut sidebar, KeyCode::Enter),
        Key::Open(String::from("1234567890"))
    );
    // Opening clears the filter, so the next pick starts from the whole tree.
    assert!(sidebar.filter.is_empty());
    assert_eq!(sidebar.items().len(), 2);
    // Short IDs and states match too.
    sidebar.paste("abcd\n");
    assert_eq!(sidebar.filter, "abcd");
    assert_eq!(ids(&sidebar), [(String::from("abcdefabcd"), 0)]);
    sidebar.filter = String::from("idle");
    assert_eq!(ids(&sidebar), [(String::from("abcdefabcd"), 0)]);
    assert_eq!(press(&mut sidebar, KeyCode::Esc), Key::Leave);
    assert!(sidebar.filter.is_empty());
    assert!(fuzzy("rvw", "reviewer") && !fuzzy("xr", "reviewer"));
}

#[test]
fn the_selection_follows_its_agent_across_updates() {
    let mut sidebar = Sidebar::new(false);
    sidebar.update(
        vec![agent("a", None, "idle"), agent("b", None, "idle")],
        false,
        none,
    );
    press(&mut sidebar, KeyCode::Down);
    assert_eq!(
        press(&mut sidebar, KeyCode::Enter),
        Key::Open(String::from("b"))
    );
    sidebar.update(
        vec![
            agent("z", None, "idle"),
            agent("a", None, "idle"),
            agent("b", None, "idle"),
        ],
        false,
        none,
    );
    assert_eq!(sidebar.selection(&sidebar.items()), Some(2));
    press(&mut sidebar, KeyCode::Down);
    assert_eq!(sidebar.selection(&sidebar.items()), Some(2));
}

#[test]
fn a_name_replaces_the_role_and_id_label_and_matches_the_filter() {
    let reply = json!({"agents": [
        {"id": "abcdef0123", "parent_id": null, "role": "reviewer", "name": "diff check",
         "state": "idle", "task_outcome": "open", "ended_at": null},
        {"id": "0123456789", "parent_id": null, "role": null, "state": "idle",
         "task_outcome": "open", "ended_at": null},
    ], "truncated": false});
    let rows = rows(&reply);
    assert_eq!(
        (rows[0].display(), rows[0].tab_label()),
        (String::from("diff check"), "diff check")
    );
    // Without a name, labels stay as they were.
    assert_eq!(
        (rows[1].display(), rows[1].tab_label()),
        (String::from("agent 01234567"), "agent")
    );
    let mut sidebar = Sidebar::new(false);
    sidebar.update(rows, false, none);
    for query in ["diff", "review", "abcdef"] {
        sidebar.filter = String::from(query);
        assert_eq!(ids(&sidebar), [(String::from("abcdef0123"), 0)], "{query}");
    }
}
