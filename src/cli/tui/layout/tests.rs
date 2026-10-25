use super::*;

fn idle(_: &str) -> bool {
    false
}

#[test]
fn views_open_in_tabs_and_splits_and_focus_instead_of_duplicating() {
    let mut layout = Layout::default();
    assert_eq!(
        layout.open("a", Place::Tab, Room::Free, idle),
        Opened::New { replaced: None }
    );
    assert_eq!(
        layout.open("b", Place::Split(Split::Columns), Room::Free, idle),
        Opened::New { replaced: None }
    );
    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(layout.active(), Some("b"));
    // A third view cannot join a full split.
    assert_eq!(
        layout.open("c", Place::Split(Split::Rows), Room::Free, idle),
        Opened::Refused("this tab already shows two views; close one first")
    );
    assert_eq!(
        layout.open("c", Place::Tab, Room::Free, idle),
        Opened::New { replaced: None }
    );
    assert_eq!((layout.tabs.len(), layout.current), (2, 1));
    assert_eq!(
        layout.open("a", Place::Tab, Room::Free, idle),
        Opened::Focused
    );
    assert_eq!((layout.current, layout.active()), (0, Some("a")));
    layout.other();
    assert_eq!(layout.active(), Some("b"));
    layout.next();
    assert_eq!(layout.active(), Some("c"));
    layout.next();
    assert_eq!(layout.current, 0);
    layout.previous();
    assert_eq!(layout.current, 1);
    layout.select(0);
    layout.select(7);
    assert_eq!(layout.current, 0);
}

#[test]
fn closing_collapses_splits_and_removes_empty_tabs() {
    let mut layout = Layout::default();
    layout.open("a", Place::Tab, Room::Free, idle);
    layout.open("b", Place::Split(Split::Rows), Room::Free, idle);
    layout.open("c", Place::Tab, Room::Free, idle);
    assert!(layout.close("c"));
    assert_eq!((layout.tabs.len(), layout.current), (1, 0));
    assert!(layout.close("b"));
    assert_eq!(layout.tabs[0].views, ["a"]);
    assert_eq!(layout.active(), Some("a"));
    assert!(!layout.close("missing"));
    assert!(layout.close("a"));
    assert!(layout.tabs.is_empty() && layout.active().is_none() && layout.count() == 0);
}

#[test]
fn at_the_cap_the_least_recent_idle_view_is_replaced() {
    let mut layout = Layout::default();
    for agent in ["a", "b", "c"] {
        layout.open(agent, Place::Tab, Room::Free, idle);
    }
    layout.focus("a");
    // "b" is least recently used, but busy: "c" is replaced instead.
    let busy = |agent: &str| agent == "b";
    assert_eq!(
        layout.open("d", Place::Tab, Room::Replace, busy),
        Opened::New {
            replaced: Some(String::from("c"))
        }
    );
    assert!(!layout.contains("c") && layout.contains("d") && layout.count() == 3);
    let all = |_: &str| true;
    assert_eq!(
        layout.open("e", Place::Tab, Room::Replace, all),
        Opened::Refused("every view has a write in flight; close one first")
    );
    assert!(matches!(
        layout.open("e", Place::Tab, Room::Full, idle),
        Opened::Refused(_)
    ));
    // Focusing an open view never needs room.
    assert_eq!(
        layout.open("a", Place::Tab, Room::Full, all),
        Opened::Focused
    );
}

#[test]
fn a_split_never_replaces_the_view_of_the_tab_it_splits() {
    let mut layout = Layout::default();
    layout.open("a", Place::Tab, Room::Free, idle);
    layout.open("b", Place::Split(Split::Columns), Room::Free, idle);
    layout.open("c", Place::Tab, Room::Free, idle);
    layout.select(0);
    // Closing "b" leaves "a", the least recently used view, alone in tab 1.
    layout.close("b");
    assert_eq!(layout.active(), Some("a"));
    assert_eq!(
        layout.open("z", Place::Split(Split::Rows), Room::Replace, idle),
        Opened::New {
            replaced: Some(String::from("c"))
        }
    );
    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(layout.tabs[0].views, ["a", "z"]);
    let mut alone = Layout::default();
    alone.open("x", Place::Tab, Room::Free, idle);
    assert_eq!(
        alone.open("y", Place::Split(Split::Rows), Room::Replace, idle),
        Opened::Refused("no view outside this tab can be replaced; close one first")
    );
}

#[test]
fn split_areas_divide_the_body_and_mark_the_active_view() {
    let mut layout = Layout::default();
    let body = Rect::new(0, 0, 80, 20);
    assert!(layout.areas(body).is_empty());
    layout.open("a", Place::Tab, Room::Free, idle);
    assert_eq!(layout.areas(body), [(String::from("a"), body, true)]);
    layout.open("b", Place::Split(Split::Columns), Room::Free, idle);
    let areas = layout.areas(body);
    assert_eq!(areas[0].1, Rect::new(0, 0, 40, 20));
    assert_eq!(areas[1].1, Rect::new(40, 0, 40, 20));
    assert!(!areas[0].2 && areas[1].2);
    layout.tabs[0].split = Split::Rows;
    let areas = layout.areas(body);
    assert_eq!(areas[1].1, Rect::new(0, 10, 80, 10));
}
