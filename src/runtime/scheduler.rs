//! Readiness admission and pure provider placement policy.
//! Runtime ownership stays in Core and Supervisor.
use super::{
    Core,
    domain::{AgentState, Control},
};
use crate::error::Result;

#[derive(Clone, Copy)]
pub(super) struct Activity {
    pub(super) class: usize,
    pub(super) busy: bool,
    pub(super) waiting: bool,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
// These independent ordering dimensions deliberately form a lexicographic eviction key.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct EvictionKey {
    resident: bool,
    coordinating: bool,
    busy: bool,
    coordinator: bool,
    recency: u64,
}
pub(super) fn eviction_priority(
    activity: impl Iterator<Item = Activity>,
    resident: bool,
    recency: u64,
) -> EvictionKey {
    let (coordinating, busy, coordinator) = activity.fold(
        (false, false, false),
        |(coordinating, busy, coordinator), activity| {
            (
                coordinating || activity.class == 0 && (activity.waiting || activity.busy),
                busy || activity.busy,
                coordinator || activity.class == 0,
            )
        },
    );
    EvictionKey {
        resident,
        coordinating,
        busy,
        coordinator,
        recency,
    }
}

// Visit every readiness class when the scheduled class has no agent.
use super::limits::READY_CLASS_COUNT;

impl Core {
    pub(super) fn ready_agents(&mut self, limit: usize) -> Result<Vec<String>> {
        // Four coordinator follow-ups, two other follow-ups, one child startup
        // and one root startup per cycle;
        // unused reservations spill to the highest ready class.
        const SERVICE: [usize; 8] = [0, 1, 0, 2, 0, 1, 0, 3];
        for id in self.store().controls()? {
            let mut agent = self.store().agent(&id)?;
            if agent.control == Some(Control::Stop) {
                agent.state = AgentState::Stopped;
                agent.control = None;
                self.store().save(&agent)?;
            }
            if agent.completion_pending.is_some() {
                self.atomic(|core| core.finish_task(&mut agent))?;
            }
        }
        let mut ids = Vec::new();
        for _ in 0..limit {
            let class = self.admission_class(&SERVICE);
            let mut chosen = self.store().claim(class)?;
            if chosen.is_none() {
                for fallback in 0..READY_CLASS_COUNT {
                    if fallback != class {
                        chosen = self.store().claim(fallback)?;
                    }
                    if chosen.is_some() {
                        break;
                    }
                }
            }
            let Some(id) = chosen else {
                break;
            };
            let mut agent = self.store().agent(&id)?;
            if (agent.auth_group.is_some() || agent.branch_unbound())
                && let Some(message) = self.store().next_message(&id)?
                && !self.auth_admit(&mut agent, &message)?
            {
                self.store().release_claim(&id)?;
                continue;
            }
            self.started(id.clone());
            ids.push(id);
        }
        Ok(ids)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures and assertions deliberately panic on failure.
mod tests {
    use super::*;
    use crate::{
        config::Lifetime,
        runtime::{domain::TaskOutcome, store::Store},
        state::State,
    };
    #[test]
    fn eviction_uses_activity_not_creation_order_and_protects_hot_coordinators() {
        let idle = Activity {
            class: 2,
            busy: false,
            waiting: false,
        };
        assert!(
            eviction_priority([idle].into_iter(), true, 10)
                < eviction_priority([idle].into_iter(), true, 20)
        );
        let coordinator = Activity {
            class: 0,
            busy: false,
            waiting: true,
        };
        assert!(
            eviction_priority([idle].into_iter(), true, 20)
                < eviction_priority([coordinator].into_iter(), true, 1)
        );
        assert!(
            eviction_priority([].into_iter(), false, 100)
                < eviction_priority([idle].into_iter(), true, 1)
        );
    }

    #[test]
    fn parent_completion_cannot_resurrect_a_child_from_the_scheduler_snapshot() {
        let temp = crate::runtime::test_directory();
        let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
        let mut parent = crate::runtime::test_support::AgentBuilder::new("parent")
            .configure(|agent| {
                agent.auth = "test".into();
                agent.model = "test".into();
                agent.effort = "high".into();
                agent.cwd = temp.path().into();
                agent.instructions = "test".into();
                agent.state = AgentState::Stopped;
                agent.paused = true;
                agent.completion_pending = Some("done".into());
            })
            .build();
        store.save(&parent).unwrap();
        parent.id = "child".into();
        parent.parent_id = Some("parent".into());
        parent.lifetime = Lifetime::WithParent;
        parent.paused = false;
        parent.state = AgentState::Starting;
        parent.completion_pending = None;
        store.save(&parent).unwrap();
        store.enqueue("queued", "human", "child", "work").unwrap();
        let mut core = crate::runtime::test_support::CoreBuilder::from_store(
            temp.path(),
            store,
            State::new(temp.path().join("state")).unwrap(),
            crate::runtime::test_support::daemon_lock(&temp.path().join("state")),
            crate::launch::test_cache().unwrap(),
        )
        .build();
        assert!(core.ready_agents(64).unwrap().is_empty());
        let child = core.store().agent("child").unwrap();
        assert!(child.expired);
        assert_eq!(child.state, AgentState::Stopped);
        assert!(core.store().next_message("child").unwrap().is_none());
        assert_eq!(
            core.store().agent("parent").unwrap().task_outcome,
            TaskOutcome::Completed
        );
    }
}
