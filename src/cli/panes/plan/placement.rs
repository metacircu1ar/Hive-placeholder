//! Placement for this component.
use super::Planner;
use super::{
    Action, History, Home, Instant, OPENS_PER_PASS, Pass, Placement, Row, home, short, wants_pane,
};

impl Planner<'_> {
    pub(super) fn plan(
        &mut self,
        rows: &[Row],
        history: &mut History,
        mut budget: usize,
        now: Instant,
        pass: &mut Pass,
    ) {
        let mut opened = 0;
        for row in rows {
            if budget == 0 || opened >= OPENS_PER_PASS {
                break;
            }
            if self.open.contains_key(row.id.as_str()) || !wants_pane(history.state(&row.id), now) {
                continue;
            }
            let action = match self.place(row, history) {
                Placement::Open(action) => action,
                Placement::Pending => {
                    pass.again = true;
                    continue;
                }
                Placement::Wait => continue,
            };
            history
                .records
                .entry(row.id.clone())
                .or_default()
                .opening(&action, now);
            pass.actions.push(action);
            budget -= 1;
            opened += 1;
        }
    }

    pub(super) fn fits(&self, tab: u64) -> bool {
        self.per_tab.get(&tab).copied().unwrap_or_default() < self.limits.per_tab
    }

    pub(super) fn stack(&mut self, row: &Row, tab: u64) -> Action {
        *self.per_tab.entry(tab).or_default() += 1;
        Action::OpenPane {
            agent: row.id.clone(),
            tab,
            stacked: self.limits.stacks,
        }
    }

    /// Where this agent's pane opens, or why it waits.
    pub(super) fn place(&mut self, row: &Row, history: &History) -> Placement {
        match home(history, &row.id, self.open, self.tabs, &self.pending_tabs) {
            Home::Pending => Placement::Pending,
            Home::Wait => Placement::Wait,
            Home::Tab(tab) if self.fits(tab) => Placement::Open(self.stack(row, tab)),
            Home::Tab(_) => {
                // Siblings past a full tab share one overflow tab per parent.
                let parent = row.parent.clone().unwrap_or_else(|| row.id.clone());
                if self.pending_overflow.contains(&parent) {
                    return Placement::Pending;
                }
                let shared = history
                    .overflow
                    .get(&parent)
                    .copied()
                    .filter(|tab| self.tabs.contains(tab));
                match shared {
                    Some(tab) if self.fits(tab) => Placement::Open(self.stack(row, tab)),
                    _ => {
                        self.pending_tabs.insert(row.id.clone());
                        self.pending_overflow.insert(parent.clone());
                        Placement::Open(Action::OpenTab {
                            agent: row.id.clone(),
                            name: format!("{} children", short(&parent)),
                            overflow: Some(parent),
                        })
                    }
                }
            }
            Home::None => {
                self.pending_tabs.insert(row.id.clone());
                // A tab keeps the label it opened with; a later rename shows in
                // its view and the controller's list.
                Placement::Open(Action::OpenTab {
                    agent: row.id.clone(),
                    name: row.name.clone().unwrap_or_else(|| {
                        row.role.as_deref().map_or_else(
                            || short(&row.id).to_owned(),
                            |role| format!("{role} {}", short(&row.id)),
                        )
                    }),
                    overflow: None,
                })
            }
        }
    }
}
