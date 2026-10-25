//! One snapshot and one bottom-up forest traversal, never one descendants scan per root.
use super::{Queries, api, queries};
use crate::error::{Error, Result};
use std::collections::{HashMap, VecDeque};

struct Node {
    id: String,
    parent_id: Option<String>,
    parent: Option<usize>,
    children: usize,
    role: Option<String>,
    name: Option<String>,
    idle: Option<u64>,
    excluded: bool,
    paused: bool,
    held: Option<api::RequiredBuild>,
    count: usize,
    preview: Vec<String>,
}
impl Queries {
    pub(crate) fn idle_since(&self, id: &str) -> Result<Option<u64>> {
        let at: Option<i64> =
            self.query_row_cached(queries::AGENT_IDLE_SINCE, [id], |row| row.get(0))?;
        at.map(|value| {
            u64::try_from(value).map_err(|cause| {
                Error::database_error("Invalid activity timestamp.").with_source(cause)
            })
        })
        .transpose()
    }

    #[expect(
        clippy::expect_used,
        reason = "Stalled group records contain JSON-compatible fields."
    )]
    pub(crate) fn stalled_groups(
        &self,
        args: &api::AgentsArgs,
        now: u64,
    ) -> Result<api::StalledPage> {
        let seconds = args.validate_stalled()?;
        let limit = usize::try_from(api::pagination::rows(api::pagination::number(
            &args.limit,
            crate::runtime::limits::DEFAULT_HISTORY_PAGE_ROWS,
            "/limit",
        )?)?)
        .map_err(super::numeric)?;
        let mut page = api::StalledPage {
            groups: vec![],
            observed_at: now,
            stalled_seconds: seconds,
            next_cursor: None,
        };
        let Some(before) = now.checked_sub(seconds) else {
            return Ok(page);
        };
        let nodes = self.stall_nodes(now)?;
        let mut groups = aggregate(nodes)?;
        groups.retain(|g| {
            g.idle_since <= before
                && args
                    .stalled_after
                    .get()
                    .is_none_or(|cursor| g.cursor() > *cursor)
        });
        groups.sort_unstable_by(|a, b| {
            a.idle_since
                .cmp(&b.idle_since)
                .then_with(|| a.root_id.cmp(&b.root_id))
        });
        let mut budget = super::PageBudget::new();
        for group in groups {
            let size = serde_json::to_vec(&group)
                .expect("serializable stalled group")
                .len();
            if page.groups.len() == limit || !budget.accept(size)? {
                page.next_cursor = page.groups.last().map(api::StalledGroup::cursor);
                break;
            }
            page.groups.push(group);
        }
        Ok(page)
    }

    fn stall_nodes(&self, now: u64) -> Result<Vec<Node>> {
        self.sql()
            .cached()
            .with_rows(queries::STALLED_AGENTS, [], |rows| {
                // Keep row decoding and the per-agent activity read interleaved so
                // a corrupt row cannot be hidden by an error in a later query.
                let mut nodes = Vec::new();
                while let Some(StallRow {
                    id,
                    parent_id,
                    role,
                    paused,
                    active,
                    excluded,
                    name,
                }) = rows.next()?
                {
                    let idle = if active { None } else { self.idle_since(&id)? };
                    let held = self.agent_hold(&id)?;
                    nodes.push(Node {
                        held,
                        preview: vec![id.clone()],
                        id,
                        parent_id,
                        parent: None,
                        children: 0,
                        role,
                        name,
                        idle,
                        excluded: (excluded && held.is_none())
                            || active
                            || idle.is_none_or(|at| at > now),
                        paused,
                        count: 1,
                    });
                }
                Ok(nodes)
            })
    }
}

#[expect(
    clippy::expect_used,
    reason = "Only eligible nodes with an activity timestamp enter the retained root groups."
)]
fn aggregate(mut nodes: Vec<Node>) -> Result<Vec<api::StalledGroup>> {
    let indexes: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();
    for i in 0..nodes.len() {
        let parent = nodes[i]
            .parent_id
            .as_ref()
            .and_then(|id| indexes.get(id))
            .copied();
        nodes[i].parent = parent;
        if let Some(p) = parent {
            nodes[p].children += 1;
        }
    }
    let mut ready: VecDeque<_> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.children == 0)
        .map(|(i, _)| i)
        .collect();
    let mut groups = Vec::new();
    let mut processed = 0;
    while let Some(i) = ready.pop_front() {
        processed += 1;
        let child_preview = std::mem::take(&mut nodes[i].preview);
        if let Some(p) = nodes[i].parent {
            nodes[p].count += nodes[i].count;
            nodes[p].idle = nodes[p].idle.max(nodes[i].idle);
            nodes[p].excluded |= nodes[i].excluded;
            nodes[p].paused |= nodes[i].paused;
            nodes[p].held = nodes[p].held.or(nodes[i].held);
            nodes[p].preview.extend(child_preview);
            nodes[p].preview.sort_unstable();
            nodes[p].preview.truncate(api::STALLED_PREVIEW);
            nodes[p].children -= 1;
            if nodes[p].children == 0 {
                ready.push_back(p);
            }
        } else if !nodes[i].excluded {
            let root = &mut nodes[i];
            groups.push(api::StalledGroup {
                held_for_build: root.held,
                root_id: std::mem::take(&mut root.id),
                role: root.role.take(),
                name: root.name.take(),
                idle_since: root.idle.expect("eligible activity"),
                open_count: root.count,
                open_preview: child_preview,
                any_paused: root.paused,
            });
        }
    }
    if processed != nodes.len() {
        return Err(Error::database_error(
            "Live agent parent links contain a cycle.",
        ));
    }
    Ok(groups)
}

super::rows::record! {
    struct StallRow {
        id: String = 0,
        parent_id: Option<String> = 1,
        role: Option<String> = 2,
        paused: bool = 3,
        active: bool = 4,
        excluded: bool = 5,
        name: Option<String> = 6,
    }
}
