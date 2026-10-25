//! Updates for the Console view.
use super::{
    Answer, CLOSED_POLL, Console, Exit, Feed, Focus, Instant, Opened, Overlay, Place, Room, View,
    sidebar,
};

impl Console<'_> {
    pub(super) fn feed(&mut self, update: Feed) {
        match update {
            Feed::Tree(reply) => {
                let views = &self.views;
                self.sidebar.update(
                    sidebar::rows(&reply),
                    super::super::views::read::<super::super::views::TreeMetadata>(&reply)
                        .truncated,
                    |id| views.get(id).and_then(View::outcome).map(str::to_owned),
                );
                if self.attention.observe(self.sidebar.agents()) > 0 {
                    self.notify = Some(self.attention.entries.len());
                }
                (self.connected, self.waiting) = (true, false);
            }
            Feed::Waiting => self.waiting = true,
            Feed::Disconnected => self.connected = false,
            Feed::Refused(_) => {}
        }
    }

    pub(super) fn answer(&mut self, answer: Answer, now: Instant) {
        let (request, result) = match answer {
            Answer::Roles(result) => {
                if let Overlay::Spawn(dialog) = &mut self.overlay {
                    match result {
                        Ok(reply) => dialog.roles(&reply),
                        Err(problem) => dialog.roles_failed(&problem),
                    }
                }
                return;
            }
            Answer::Spawned { id, result } => (id, result),
        };
        // The reply belongs to the open dialog only if that dialog sent it. A
        // spawn from a dialog that was left or replaced still reports, without
        // moving the user from where they are now.
        let dialog = match &mut self.overlay {
            Overlay::Spawn(dialog) if dialog.request.as_deref() == Some(request.as_str()) => {
                Some(dialog)
            }
            _ => None,
        };
        let id = match result {
            Err(error) => {
                if let Some(dialog) = dialog {
                    dialog.failed(&error);
                } else {
                    let error: super::super::views::Failure<'_> = super::super::views::read(&error);
                    let code = error
                        .code
                        .unwrap_or(crate::error::ErrorCode::Internal)
                        .as_str();
                    let message = error.message.unwrap_or_default();
                    self.status = format!("spawn failed: {code}: {message}");
                }
                return;
            }
            Ok(reply) => {
                super::super::views::read::<super::super::views::AgentIdentity<'_>>(&reply)
                    .agent_id
                    .unwrap_or_default()
                    .to_owned()
            }
        };
        let elsewhere = dialog.is_none();
        let started = if self.args.root.is_some() {
            format!("started {id}: outside this TUI's scope; reopen it with hivectl attach {id}")
        } else {
            format!("started {}", sidebar::short(&id))
        };
        let (overlay, focus) = (
            std::mem::replace(&mut self.overlay, Overlay::None),
            self.focus,
        );
        let opened = !id.is_empty() && self.open(&id, Some(Place::Tab), now);
        if elsewhere {
            (self.overlay, self.focus) = (overlay, focus);
        }
        // A view refused at the cap still reports the new agent's ID.
        self.status = if opened || id.is_empty() {
            started
        } else {
            format!("{started}; {}", self.status)
        };
    }

    /// Apply view updates and act on views that closed or switched.
    pub(super) fn pump(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let ids: Vec<String> = self.views.keys().cloned().collect();
        for id in ids {
            let Some(view) = self.views.get_mut(&id) else {
                continue;
            };
            changed |= view.pump();
            view.tick(now);
            let Some(exit) = view.take_exit() else {
                continue;
            };
            changed = true;
            match exit {
                Exit::Quit => self.close(&id, Some(now)),
                Exit::Attach(next) => {
                    self.open(&next, None, now);
                }
                Exit::Refused(reply) => {
                    // The refused attach was answered: no request stays open.
                    self.close(&id, None);
                    let code = super::super::views::error_code(&reply).as_str();
                    self.status = format!("cannot open {}: {code}", sidebar::short(&id));
                }
            }
        }
        changed
    }

    /// Closed views whose long-polls may still be in flight.
    pub(super) fn lingering(&mut self, now: Instant) -> usize {
        self.closed
            .retain(|closed| now.duration_since(*closed) < CLOSED_POLL);
        self.closed.len()
    }

    /// Open or focus an agent's view; `false` when it was refused.
    pub(super) fn open(&mut self, agent: &str, place: Option<Place>, now: Instant) -> bool {
        let lingering = self.lingering(now);
        let limit = usize::from(self.args.limit);
        let room = if self.views.len() + lingering < limit {
            Room::Free
        } else if lingering == 0 {
            Room::Replace
        } else {
            Room::Full
        };
        let views = &self.views;
        let opened = self
            .layout
            .open(agent, place.unwrap_or(Place::Tab), room, |id| {
                views.get(id).is_some_and(View::busy)
            });
        match opened {
            Opened::Focused => {}
            Opened::New { replaced } => {
                if let Some(old) = replaced {
                    self.views.remove(&old);
                    self.closed.push(now);
                }
                let mut view = View::open(&self.root, agent, self.mode());
                let prefix = self.prefix();
                view.set_hint(format!(
                    "Enter send · Alt-Enter queue · Esc Esc interrupt · Tab focus · {prefix} keys"
                ));
                self.views.insert(agent.to_owned(), view);
            }
            Opened::Refused(problem) => {
                problem.clone_into(&mut self.status);
                return false;
            }
        }
        (self.focus, self.overlay) = (Focus::View, Overlay::None);
        true
    }

    /// Close a view; `now` when its follower may still be in a long-poll.
    /// Open an agent from the questions list, its question focused as soon as
    /// it is shown.
    pub(super) fn open_question(&mut self, agent: &str, now: Instant) {
        if self.open(agent, None, now)
            && let Some(view) = self.views.get_mut(agent)
        {
            view.focus_card();
        }
    }

    /// Open an agent picked in the sidebar, where a pending `t`/`v`/`h` says.
    pub(super) fn open_picked(&mut self, agent: &str, now: Instant) {
        let place = self.place.take();
        if self.open(agent, place, now) {
            self.status = self.hint();
        }
    }

    pub(super) fn close(&mut self, agent: &str, now: Option<Instant>) {
        if self.views.remove(agent).is_some()
            && let Some(now) = now
        {
            self.closed.push(now);
        }
        self.layout.close(agent);
        if self.layout.count() == 0 {
            self.focus = Focus::Sidebar;
        }
    }

    pub(super) fn active_view(&mut self) -> Option<&mut View> {
        let active = self.layout.active()?.to_owned();
        self.views.get_mut(&active)
    }
}
