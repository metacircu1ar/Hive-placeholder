use super::*;

fn row(id: &str, parent: Option<&str>) -> Row {
    Row {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        role: Some(String::from("worker")),
        name: None,
        state: String::from("idle"),
    }
}

fn pane(agent: &str, tab: u64) -> Pane {
    Pane {
        agent: agent.to_owned(),
        tab,
    }
}

const LIMITS: Limits = Limits {
    max_panes: 16,
    per_tab: ZELLIJ_PER_TAB,
    stacks: true,
};

/// Run a pass and record its actions as the loop would, with new tabs numbered
/// from `next_tab`; returns the actions and the panes now present.
fn run(
    rows: &[Row],
    panes: &mut Vec<Pane>,
    history: &mut History,
    now: Instant,
    next_tab: &mut u64,
) -> Pass {
    let pass = reconcile(rows, panes, history, &LIMITS, now);
    for action in &pass.actions {
        match action {
            Action::OpenTab { agent, .. } => {
                *next_tab += 1;
                history.opened(action, Some(*next_tab), now);
                panes.push(pane(agent, *next_tab));
            }
            Action::OpenPane { agent, tab, .. } => {
                history.opened(action, None, now);
                panes.push(pane(agent, *tab));
            }
        }
    }
    pass
}

#[test]
fn roots_get_tabs_and_descendants_join_their_ancestors_tab() {
    let rows = [
        row("root", None),
        row("child", Some("root")),
        row("grandchild", Some("child")),
    ];
    let (mut panes, mut history, mut next) = (Vec::new(), History::default(), 0);
    let now = Instant::now();
    let first = run(&rows, &mut panes, &mut history, now, &mut next);
    assert_eq!(
        first.actions,
        [Action::OpenTab {
            agent: String::from("root"),
            name: String::from("worker root"),
            overflow: None
        }]
    );
    // The children wait for the root's new tab, then stack into it.
    assert!(first.again);
    // Both descendants join the root's tab once it exists.
    let second = run(&rows, &mut panes, &mut history, now, &mut next);
    assert_eq!(
        second.actions,
        [
            Action::OpenPane {
                agent: String::from("child"),
                tab: 1,
                stacked: true
            },
            Action::OpenPane {
                agent: String::from("grandchild"),
                tab: 1,
                stacked: true
            }
        ]
    );
    let third = reconcile(&rows, &panes, &mut history, &LIMITS, now);
    assert!(third.actions.is_empty());
    let depths: Vec<_> = third
        .status
        .iter()
        .map(|status| (status.depth, status.pane))
        .collect();
    assert_eq!(depths, [(0, "open"), (1, "open"), (2, "open")]);
    // Nothing is opened twice.
    assert!(
        run(&rows, &mut panes, &mut history, now, &mut next)
            .actions
            .is_empty()
    );
}

#[test]
fn a_full_tab_overflows_into_one_shared_children_tab_and_the_limit_lists_the_rest() {
    let mut rows = vec![row("root", None)];
    rows.extend((0..8).map(|index| row(&format!("c{index}"), Some("root"))));
    let mut panes = vec![pane("root", 1)];
    panes.extend((0..5).map(|index| pane(&format!("c{index}"), 1)));
    let (mut history, mut next) = (History::default(), 1);
    let now = Instant::now();
    let first = run(&rows, &mut panes, &mut history, now, &mut next);
    assert_eq!(
        first.actions,
        [Action::OpenTab {
            agent: String::from("c5"),
            name: String::from("root children"),
            overflow: Some(String::from("root"))
        }]
    );
    assert!(first.again);
    // The other siblings join that overflow tab.
    let second = run(&rows, &mut panes, &mut history, now, &mut next);
    assert_eq!(
        second.actions,
        [
            Action::OpenPane {
                agent: String::from("c6"),
                tab: 2,
                stacked: true
            },
            Action::OpenPane {
                agent: String::from("c7"),
                tab: 2,
                stacked: true
            }
        ]
    );
    // With a lower limit, agents past it are listed as not opened.
    let limited = reconcile(
        &rows,
        &panes[..6],
        &mut History::default(),
        &Limits {
            max_panes: 7,
            ..LIMITS
        },
        now,
    );
    assert_eq!(limited.actions.len(), 1);
    let listed = limited
        .status
        .iter()
        .filter(|status| status.pane == "not opened")
        .count();
    assert_eq!(listed, 2);
}

#[test]
fn existing_panes_are_adopted_and_panes_outside_the_tree_are_ignored() {
    let rows = [row("root", None), row("child", Some("root"))];
    let panes = [pane("root", 3), pane("child", 3), pane("stranger", 3)];
    let mut history = History::default();
    let pass = reconcile(
        &rows,
        &panes,
        &mut history,
        &Limits {
            max_panes: 2,
            ..LIMITS
        },
        Instant::now(),
    );
    assert!(pass.actions.is_empty());
    assert_eq!(history.state("stranger"), None);
    assert!(pass.status.iter().all(|status| status.pane == "open"));
}

#[test]
fn a_dropped_stacked_pane_is_reopened_unstacked_once_then_counts_as_failed() {
    let rows = [row("root", None), row("child", Some("root"))];
    let panes = vec![pane("root", 1)];
    let mut history = History::default();
    let start = Instant::now();
    let pass = reconcile(&rows, &panes, &mut history, &LIMITS, start);
    assert_eq!(pass.actions.len(), 1);
    history.opened(&pass.actions[0], None, start);
    // Still within the verification window: nothing happens.
    assert!(
        reconcile(&rows, &panes, &mut history, &LIMITS, start + VERIFY / 2)
            .actions
            .is_empty()
    );
    let later = start + VERIFY + Duration::from_millis(1);
    let retry = reconcile(&rows, &panes, &mut history, &LIMITS, later);
    assert_eq!(
        retry.actions,
        [Action::OpenPane {
            agent: String::from("child"),
            tab: 1,
            stacked: false
        }]
    );
    let gone = later + VERIFY + Duration::from_millis(1);
    reconcile(&rows, &panes, &mut history, &LIMITS, gone);
    assert!(matches!(
        history.state("child"),
        Some(PaneState::Failed { count: 1, .. })
    ));
}

#[test]
fn a_pane_the_user_closes_stays_closed_until_reopened() {
    let rows = [row("root", None)];
    let mut history = History::default();
    let start = Instant::now();
    reconcile(&rows, &[pane("root", 1)], &mut history, &LIMITS, start);
    let later = start + EARLY_EXIT + Duration::from_secs(1);
    let closed = reconcile(&rows, &[], &mut history, &LIMITS, later);
    assert!(closed.actions.is_empty());
    assert_eq!(closed.status[0].pane, "closed by you");
    assert!(
        reconcile(&rows, &[], &mut history, &LIMITS, later)
            .actions
            .is_empty()
    );
    history.reopen();
    assert_eq!(
        reconcile(&rows, &[], &mut history, &LIMITS, later)
            .actions
            .len(),
        1
    );
}

#[test]
fn failed_opens_retry_after_each_backoff_then_stop() {
    let rows = [row("root", None)];
    let mut history = History::default();
    let mut now = Instant::now();
    for attempt in 0..=RETRIES.len() {
        let pass = reconcile(&rows, &[], &mut history, &LIMITS, now);
        assert_eq!(pass.actions.len(), 1, "attempt {attempt}");
        // The pane appears, then exits at once while the agent still lives.
        history.opened(&pass.actions[0], Some(9), now);
        reconcile(&rows, &[pane("root", 9)], &mut history, &LIMITS, now);
        let exited = now + Duration::from_secs(1);
        assert!(
            reconcile(&rows, &[], &mut history, &LIMITS, exited)
                .actions
                .is_empty()
        );
        let Some(wait) = RETRIES.get(attempt) else {
            break;
        };
        // No retry before the backoff has passed.
        assert!(
            reconcile(&rows, &[], &mut history, &LIMITS, exited + *wait / 2)
                .actions
                .is_empty()
        );
        now = exited + *wait;
    }
    let pass = reconcile(
        &rows,
        &[],
        &mut history,
        &LIMITS,
        now + Duration::from_secs(3600),
    );
    assert!(pass.actions.is_empty());
    assert_eq!(pass.status[0].pane, "failed");
    // A failed action counts the same way.
    let mut history = History::default();
    history.failed("root", now);
    assert_eq!(
        reconcile(&rows, &[], &mut history, &LIMITS, now).status[0].pane,
        "retrying"
    );
}

#[test]
fn a_live_child_of_an_ended_parent_opens_in_the_roots_tab() {
    let mut history = History::default();
    let now = Instant::now();
    let rows = [
        row("root", None),
        row("middle", Some("root")),
        row("leaf", Some("middle")),
    ];
    let panes = [pane("root", 4), pane("middle", 4), pane("leaf", 4)];
    reconcile(&rows, &panes, &mut history, &LIMITS, now);
    // The middle agent ends and its pane closes; a new grandchild appears.
    let rows = [
        row("root", None),
        row("leaf", Some("middle")),
        row("late", Some("middle")),
    ];
    let panes = [pane("root", 4), pane("leaf", 4)];
    let pass = reconcile(&rows, &panes, &mut history, &LIMITS, now);
    assert_eq!(
        pass.actions,
        [Action::OpenPane {
            agent: String::from("late"),
            tab: 4,
            stacked: true
        }]
    );
    // Once the whole subtree has ended, its records are pruned.
    reconcile(&[], &[], &mut history, &LIMITS, now);
    assert!(history.records.is_empty());
    assert_eq!(history.deadline(), None);
}

#[test]
fn deadlines_come_only_from_opening_and_retrying_panes() {
    let rows = [row("root", None)];
    let mut history = History::default();
    let now = Instant::now();
    // An adopted, open pane never makes the loop due.
    reconcile(&rows, &[pane("root", 1)], &mut history, &LIMITS, now);
    assert_eq!(history.deadline(), None);
    let mut history = History::default();
    let pass = reconcile(&rows, &[], &mut history, &LIMITS, now);
    assert_eq!(history.deadline(), Some(now + VERIFY));
    history.opened(&pass.actions[0], Some(1), now);
    reconcile(&rows, &[pane("root", 1)], &mut history, &LIMITS, now);
    assert_eq!(history.deadline(), None);
}

#[test]
fn ended_ancestors_keep_only_ancestry_and_children_wait_for_their_ancestors_tab() {
    let now = Instant::now();
    let mut history = History::default();
    // The root's tab is planned but its open fails: its child waits rather than
    // taking a tab of its own.
    let rows = [row("root", None), row("child", Some("root"))];
    let pass = reconcile(&rows, &[], &mut history, &LIMITS, now);
    assert_eq!(pass.actions.len(), 1);
    history.failed("root", now);
    let waiting = reconcile(&rows, &[], &mut history, &LIMITS, now);
    assert!(waiting.actions.is_empty() && !waiting.again);
    // Once the root ends while still opening, its stale state no longer blocks
    // the budget or the child.
    let mut history = History::default();
    let pass = reconcile(&rows, &[], &mut history, &LIMITS, now);
    history.opened(&pass.actions[0], None, now);
    let rows = [row("child", Some("root"))];
    let pass = reconcile(
        &rows,
        &[],
        &mut history,
        &Limits {
            max_panes: 1,
            ..LIMITS
        },
        now,
    );
    assert_eq!(pass.actions.len(), 1);
    assert_eq!(history.state("root"), None);
}

#[test]
fn a_splitting_backend_caps_tabs_at_four_and_never_retries_unstacked() {
    let limits = Limits {
        max_panes: 16,
        per_tab: 4,
        stacks: false,
    };
    let mut rows = vec![row("root", None)];
    rows.extend((0..4).map(|index| row(&format!("c{index}"), Some("root"))));
    let mut panes = vec![pane("root", 1)];
    panes.extend((0..3).map(|index| pane(&format!("c{index}"), 1)));
    let mut history = History::default();
    let start = Instant::now();
    let pass = reconcile(&rows, &panes, &mut history, &limits, start);
    // The tab already holds four agent panes: the fifth overflows.
    assert_eq!(
        pass.actions,
        [Action::OpenTab {
            agent: String::from("c3"),
            name: String::from("root children"),
            overflow: Some(String::from("root"))
        }]
    );
    // A split that never appears is a failed open, not an unstacked retry.
    let rows = [row("root", None), row("child", Some("root"))];
    let mut history = History::default();
    let pass = reconcile(&rows, &[pane("root", 1)], &mut history, &limits, start);
    assert_eq!(
        pass.actions,
        [Action::OpenPane {
            agent: String::from("child"),
            tab: 1,
            stacked: false
        }]
    );
    let later = start + VERIFY + Duration::from_millis(1);
    assert!(
        reconcile(&rows, &[pane("root", 1)], &mut history, &limits, later)
            .actions
            .is_empty()
    );
    assert!(matches!(
        history.state("child"),
        Some(PaneState::Failed { count: 1, .. })
    ));
}

#[test]
fn a_named_root_tab_takes_its_name() {
    let mut named = row("root", None);
    named.name = Some(String::from("release lead"));
    let (mut panes, mut history, mut next) = (Vec::new(), History::default(), 0);
    let pass = run(
        &[named],
        &mut panes,
        &mut history,
        Instant::now(),
        &mut next,
    );
    assert_eq!(
        pass.actions,
        [Action::OpenTab {
            agent: String::from("root"),
            name: String::from("release lead"),
            overflow: None
        }]
    );
}
