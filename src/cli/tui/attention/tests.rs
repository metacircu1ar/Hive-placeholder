use super::*;
use ratatui::crossterm::event::KeyModifiers;

fn asking(id: &str, approval: Option<&str>) -> Agent {
    Agent {
        id: id.to_owned(),
        parent: None,
        role: None,
        name: None,
        state: String::from("idle"),
        sandbox: String::from("workspace-write"),
        outcome: String::from("open"),
        waiting: None,
        approval: approval.map(str::to_owned),
        ended: false,
    }
}

#[test]
fn entries_keep_first_seen_order_and_leave_when_answered() {
    let mut attention = Attention::default();
    assert_eq!(
        attention.observe(&[asking("b", Some("q2")), asking("a", Some("q1"))]),
        2
    );
    // Feed order changes; first-seen order does not.
    assert_eq!(
        attention.observe(&[
            asking("a", Some("q1")),
            asking("b", Some("q2")),
            asking("c", Some("q3"))
        ]),
        1
    );
    let agents: Vec<&str> = attention.entries.iter().map(|e| e.agent.as_str()).collect();
    assert_eq!(agents, ["b", "a", "c"]);
    // An answered question leaves; a new question from the same agent is new.
    assert_eq!(
        attention.observe(&[
            asking("a", Some("q4")),
            asking("b", None),
            asking("c", Some("q3"))
        ]),
        1
    );
    let agents: Vec<&str> = attention.entries.iter().map(|e| e.agent.as_str()).collect();
    assert_eq!(agents, ["c", "a"]);
    let mut ended = asking("c", Some("q3"));
    ended.ended = true;
    attention.observe(&[ended]);
    assert!(attention.entries.is_empty());
}

#[test]
fn keys_move_open_and_leave() {
    let mut attention = Attention::default();
    attention.observe(&[asking("a", Some("q1")), asking("b", Some("q2"))]);
    let press =
        |attention: &mut Attention, code| attention.key(&KeyEvent::new(code, KeyModifiers::NONE));
    assert_eq!(press(&mut attention, KeyCode::Down), Key::None);
    assert_eq!(press(&mut attention, KeyCode::Down), Key::None);
    assert_eq!(
        press(&mut attention, KeyCode::Enter),
        Key::Open(String::from("b"))
    );
    assert_eq!(press(&mut attention, KeyCode::Up), Key::None);
    assert_eq!(
        press(&mut attention, KeyCode::Enter),
        Key::Open(String::from("a"))
    );
    assert_eq!(press(&mut attention, KeyCode::Esc), Key::Leave);
    attention.observe(&[]);
    assert_eq!(press(&mut attention, KeyCode::Enter), Key::None);
}

#[test]
fn the_notifier_writes_the_fixed_payload_once_per_call() {
    let mut sink = Vec::new();
    notify(&mut sink, 1).unwrap();
    assert_eq!(sink, b"\x07\x1b]9;Hive: 1 agent needs you\x07");
    let mut sink = Vec::new();
    notify(&mut sink, 3).unwrap();
    assert_eq!(sink, b"\x07\x1b]9;Hive: 3 agents need you\x07");
}
