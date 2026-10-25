//! The pure reconciler: tree rows, existing panes and per-agent history in,
//! pane actions and a status list out.
mod placement;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// zellij agent panes per tab, stacked, before descendants overflow into a
/// children tab.
pub(super) const ZELLIJ_PER_TAB: usize = 6;
/// Pane opens per pass, so a large tree opens gradually.
pub(super) const OPENS_PER_PASS: usize = 8;
/// A new pane gets this long to appear in the pane list.
pub(super) const VERIFY: Duration = Duration::from_millis(1500);
/// A pane gone this soon after opening, while its agent lives, failed to open.
pub(super) const EARLY_EXIT: Duration = Duration::from_secs(5);
/// Waits before reopening after the first, second and third failed open.
pub(super) const RETRIES: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];

/// One live agent in scope, from the tree feed, in listing order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub id: String,
    pub parent: Option<String>,
    pub role: Option<String>,
    /// The display name, which labels the agent's tab instead of its role and ID.
    pub name: Option<String>,
    pub state: String,
}

/// One agent pane found in the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Pane {
    pub agent: String,
    pub tab: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// A new tab holding the agent's pane; for an overflow tab, the parent
    /// whose later children join it.
    OpenTab {
        agent: String,
        name: String,
        overflow: Option<String>,
    },
    /// A pane for the agent in an existing tab.
    OpenPane {
        agent: String,
        tab: u64,
        stacked: bool,
    },
}

/// What happened to an agent's pane, as the status list shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneState {
    Opening {
        at: Instant,
        stacked: bool,
    },
    Open {
        at: Instant,
    },
    /// The user closed it; it stays closed until reopened by key.
    Closed,
    /// Opens failed; the next try waits until `retry`, if any is left.
    Failed {
        count: usize,
        retry: Option<Instant>,
    },
}

#[derive(Debug, Default)]
struct Record {
    parent: Option<String>,
    /// The tab the agent's pane was last seen or opened in.
    tab: Option<u64>,
    pane: Option<PaneState>,
    /// Failed opens since the pane last stayed up.
    failures: usize,
}

impl Record {
    fn fail(&mut self, now: Instant) {
        self.failures += 1;
        self.pane = Some(PaneState::Failed {
            count: self.failures,
            retry: RETRIES.get(self.failures - 1).map(|wait| now + *wait),
        });
    }
    /// An action was planned or run: the pane is opening in this tab.
    fn opening(&mut self, action: &Action, now: Instant) {
        let stacked = match action {
            Action::OpenTab { .. } => false,
            Action::OpenPane { tab, stacked, .. } => {
                self.tab = Some(*tab);
                *stacked
            }
        };
        self.pane = Some(PaneState::Opening { at: now, stacked });
    }
}

/// Per-agent memory across passes, pruned to the live tree.
#[derive(Debug, Default)]
pub(super) struct History {
    records: HashMap<String, Record>,
    /// Each parent's overflow tab for children past a full tab.
    overflow: HashMap<String, u64>,
}

/// One status-list line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Status {
    pub depth: usize,
    pub id: String,
    pub role: Option<String>,
    pub name: Option<String>,
    pub state: String,
    pub pane: &'static str,
}

#[derive(Debug, Default)]
pub(super) struct Pass {
    pub actions: Vec<Action>,
    /// Some agents wait for a tab opened in this pass: run another pass after.
    pub again: bool,
    pub status: Vec<Status>,
}

pub(super) struct Limits {
    pub max_panes: usize,
    /// Agent panes per tab before descendants overflow into a children tab.
    pub per_tab: usize,
    /// The backend can stack panes (zellij); otherwise each pane is a split.
    pub stacks: bool,
}

impl History {
    /// Reopen every pane the user closed or that stopped retrying.
    pub(super) fn reopen(&mut self) {
        for record in self.records.values_mut() {
            if matches!(
                record.pane,
                Some(PaneState::Closed | PaneState::Failed { .. })
            ) {
                record.pane = None;
            }
        }
    }

    /// Record the outcome of an action the loop just ran.
    pub(super) fn opened(&mut self, action: &Action, tab: Option<u64>, now: Instant) {
        let agent = match action {
            Action::OpenTab {
                agent, overflow, ..
            } => {
                if let (Some(parent), Some(tab)) = (overflow, tab) {
                    self.overflow.insert(parent.clone(), tab);
                }
                agent
            }
            Action::OpenPane { agent, .. } => agent,
        };
        let record = self.records.entry(agent.clone()).or_default();
        record.opening(action, now);
        if tab.is_some() {
            record.tab = tab;
        }
    }

    /// An action failed outright: count it like a pane that never appeared.
    pub(super) fn failed(&mut self, agent: &str, now: Instant) {
        self.records.entry(agent.to_owned()).or_default().fail(now);
    }

    pub(super) fn state(&self, agent: &str) -> Option<PaneState> {
        self.records.get(agent).and_then(|record| record.pane)
    }

    /// The earliest moment a pass has work to do without a new wake.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.records
            .values()
            .filter_map(|record| match record.pane {
                Some(PaneState::Opening { at, .. }) => Some(at + VERIFY),
                Some(PaneState::Failed { retry, .. }) => retry,
                _ => None,
            })
            .min()
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Run one reconciliation pass.
pub(super) fn reconcile(
    rows: &[Row],
    panes: &[Pane],
    history: &mut History,
    limits: &Limits,
    now: Instant,
) -> Pass {
    let live: HashMap<&str, &Row> = rows.iter().map(|row| (row.id.as_str(), row)).collect();
    // Only panes of agents in this tree count; others belong to someone else.
    let open: HashMap<&str, &Pane> = panes
        .iter()
        .filter(|pane| live.contains_key(pane.agent.as_str()))
        .map(|pane| (pane.agent.as_str(), pane))
        .collect();
    let tabs: HashSet<u64> = panes.iter().map(|pane| pane.tab).collect();
    let mut per_tab: HashMap<u64, usize> = HashMap::new();
    for pane in open.values() {
        *per_tab.entry(pane.tab).or_default() += 1;
    }
    for row in rows {
        let record = history.records.entry(row.id.clone()).or_default();
        record.parent.clone_from(&row.parent);
    }
    // Ended agents keep only ancestry: their pane state no longer counts.
    for (agent, record) in &mut history.records {
        if !live.contains_key(agent.as_str()) {
            record.pane = None;
        }
    }
    let mut pass = Pass::default();
    for row in rows {
        let present = open.get(row.id.as_str()).copied();
        let record = history.records.entry(row.id.clone()).or_default();
        if let Some(pane) = present {
            record.tab = Some(pane.tab);
        }
        observe(record, present.is_some(), now, &mut pass, &row.id);
    }
    let budget = limits
        .max_panes
        .saturating_sub(open.len() + opening(history, &open));
    let mut planner = Planner {
        open: &open,
        tabs: &tabs,
        limits,
        per_tab,
        pending_tabs: HashSet::new(),
        pending_overflow: HashSet::new(),
    };
    planner.plan(rows, history, budget, now, &mut pass);
    prune(history, &live);
    pass.status = status(rows, history, &open, &live);
    pass
}

/// Where a missing pane goes in this pass.
enum Placement {
    Open(Action),
    /// Its ancestor's tab opens in this pass: run another pass after.
    Pending,
    /// Its ancestor's pane is opening or retrying: try again on a later pass.
    Wait,
}

/// Chooses where missing panes open, within the pass's budget.
struct Planner<'a> {
    open: &'a HashMap<&'a str, &'a Pane>,
    tabs: &'a HashSet<u64>,
    limits: &'a Limits,
    per_tab: HashMap<u64, usize>,
    /// Agents whose tab opens in this pass; their descendants wait for it.
    pending_tabs: HashSet<String>,
    /// Parents whose overflow tab opens in this pass.
    pending_overflow: HashSet<String>,
}

impl Planner<'_> {}

/// Update one agent's pane state from whether its pane is listed now.
fn observe(record: &mut Record, present: bool, now: Instant, pass: &mut Pass, agent: &str) {
    match (record.pane, present) {
        (Some(PaneState::Opening { at, .. } | PaneState::Open { at }), true) => {
            // A pane that stayed up past the early-exit window has succeeded.
            if now.duration_since(at) >= EARLY_EXIT {
                record.failures = 0;
            }
            record.pane = Some(PaneState::Open { at });
        }
        // Adopted from an earlier controller: treat it as open long ago.
        (None | Some(PaneState::Closed | PaneState::Failed { .. }), true) => {
            record.failures = 0;
            record.pane = Some(PaneState::Open {
                at: now.checked_sub(EARLY_EXIT).unwrap_or(now),
            });
        }
        (Some(PaneState::Opening { at, .. }), false) if now.duration_since(at) < VERIFY => {}
        // A stacked pane can be dropped without a client: try once unstacked.
        (Some(PaneState::Opening { stacked: true, .. }), false) => match record.tab {
            Some(tab) => {
                let action = Action::OpenPane {
                    agent: agent.to_owned(),
                    tab,
                    stacked: false,
                };
                record.opening(&action, now);
                pass.actions.push(action);
            }
            None => record.fail(now),
        },
        (Some(PaneState::Opening { .. }), false) => record.fail(now),
        (Some(PaneState::Open { at }), false) if now.duration_since(at) < EARLY_EXIT => {
            record.fail(now);
        }
        (Some(PaneState::Open { .. }), false) => {
            record.failures = 0;
            record.pane = Some(PaneState::Closed);
        }
        (_, false) => {}
    }
}

fn wants_pane(state: Option<PaneState>, now: Instant) -> bool {
    match state {
        None => true,
        Some(PaneState::Failed {
            retry: Some(retry), ..
        }) => now >= retry,
        Some(_) => false,
    }
}

fn opening(history: &History, open: &HashMap<&str, &Pane>) -> usize {
    history
        .records
        .iter()
        .filter(|(agent, record)| {
            !open.contains_key(agent.as_str())
                && matches!(record.pane, Some(PaneState::Opening { .. }))
        })
        .count()
}

enum Home {
    /// The tab of the nearest ancestor with a pane.
    Tab(u64),
    /// That ancestor's tab is being opened in this pass.
    Pending,
    /// That ancestor's pane is still opening or will be retried: wait for it,
    /// so the subtree is not scattered into tabs of its own.
    Wait,
    None,
}

/// Walk recorded parents, ended ones included, to the nearest ancestor's tab.
fn home(
    history: &History,
    agent: &str,
    open: &HashMap<&str, &Pane>,
    tabs: &HashSet<u64>,
    pending: &HashSet<String>,
) -> Home {
    let mut current = history
        .records
        .get(agent)
        .and_then(|record| record.parent.clone());
    let mut seen = HashSet::new();
    while let Some(ancestor) = current {
        if !seen.insert(ancestor.clone()) {
            break;
        }
        if pending.contains(&ancestor) {
            return Home::Pending;
        }
        if let Some(pane) = open.get(ancestor.as_str()) {
            return Home::Tab(pane.tab);
        }
        let record = history.records.get(&ancestor);
        if let Some(tab) = record.and_then(|record| record.tab)
            && tabs.contains(&tab)
        {
            return Home::Tab(tab);
        }
        if matches!(
            record.and_then(|record| record.pane),
            Some(PaneState::Opening { .. } | PaneState::Failed { retry: Some(_), .. })
        ) {
            return Home::Wait;
        }
        current = record.and_then(|record| record.parent.clone());
    }
    Home::None
}

/// Keep records of live agents and of ancestors that still link live ones.
fn prune(history: &mut History, live: &HashMap<&str, &Row>) {
    let mut linked: HashSet<String> = HashSet::new();
    for agent in live.keys() {
        let mut current = Some((*agent).to_owned());
        while let Some(id) = current {
            if !linked.insert(id.clone()) {
                break;
            }
            current = history
                .records
                .get(&id)
                .and_then(|record| record.parent.clone());
        }
    }
    history.records.retain(|agent, _| linked.contains(agent));
    history.overflow.retain(|parent, _| linked.contains(parent));
}

/// The status list after a pass's actions ran.
pub(super) fn status_list(rows: &[Row], panes: &[Pane], history: &History) -> Vec<Status> {
    let live: HashMap<&str, &Row> = rows.iter().map(|row| (row.id.as_str(), row)).collect();
    let open: HashMap<&str, &Pane> = panes
        .iter()
        .filter(|pane| live.contains_key(pane.agent.as_str()))
        .map(|pane| (pane.agent.as_str(), pane))
        .collect();
    status(rows, history, &open, &live)
}

fn status(
    rows: &[Row],
    history: &History,
    open: &HashMap<&str, &Pane>,
    live: &HashMap<&str, &Row>,
) -> Vec<Status> {
    rows.iter()
        .map(|row| {
            let mut depth = 0;
            let mut parent = row.parent.as_deref();
            while let Some(id) = parent.filter(|_| depth < 32) {
                depth += 1;
                parent = live
                    .get(id)
                    .and_then(|row| row.parent.as_deref())
                    .or_else(|| {
                        history
                            .records
                            .get(id)
                            .and_then(|record| record.parent.as_deref())
                    });
            }
            let pane = match (open.contains_key(row.id.as_str()), history.state(&row.id)) {
                (true, _) | (false, Some(PaneState::Open { .. })) => "open",
                (false, Some(PaneState::Opening { .. })) => "opening",
                (false, Some(PaneState::Closed)) => "closed by you",
                (false, Some(PaneState::Failed { retry: Some(_), .. })) => "retrying",
                (false, Some(PaneState::Failed { retry: None, .. })) => "failed",
                (false, None) => "not opened",
            };
            Status {
                depth,
                id: row.id.clone(),
                role: row.role.clone(),
                name: row.name.clone(),
                state: row.state.clone(),
                pane,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
