//! Interaction for the Console view.
use super::{
    Action, Armed, Ask, Console, Event, Focus, Instant, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, Layout, Overlay, Place, Routed, View, attention, sidebar, spawn,
};
use std::sync::Arc;

impl Console<'_> {
    /// Handle one terminal event; `false` when it changed nothing, so the
    /// frame need not be redrawn. Only mouse events can change nothing.
    pub(super) fn event(&mut self, event: &Event, now: Instant) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key, now),
            Event::Paste(text) => match &mut self.overlay {
                Overlay::Spawn(dialog) => dialog.paste(text),
                Overlay::Attention => {}
                Overlay::None if self.focus == Focus::Sidebar => self.sidebar.paste(text),
                Overlay::None => {
                    if let Some(view) = self.active_view() {
                        view.event(event);
                    }
                }
            },
            Event::Mouse(mouse) => return self.mouse(*mouse, now),
            _ => {}
        }
        true
    }

    pub(super) fn key(&mut self, key: &KeyEvent, now: Instant) {
        let armed = self.armed.take();
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.router.clear();
            self.quit(armed);
            return;
        }
        match self.router.route(key, now) {
            Routed::Pending => self.armed = armed,
            Routed::Action(action) => self.action(action, armed, now),
            Routed::Prefix => {
                if let Some(view) = self.active_view() {
                    view.event(&Event::Key(*key));
                }
            }
            Routed::Dropped => {}
            Routed::Key => self.focused_key(key, now),
        }
    }

    pub(super) fn focused_key(&mut self, key: &KeyEvent, now: Instant) {
        match &mut self.overlay {
            Overlay::Spawn(dialog) => match dialog.key(key) {
                spawn::Key::Submit { id, request } => {
                    // The worker ends only with the process; a send cannot fail first.
                    let _ = self.asks.send(Ask::Spawn { id, request });
                }
                spawn::Key::Cancel => self.overlay = Overlay::None,
                spawn::Key::None => {}
            },
            Overlay::Attention => match self.attention.key(key) {
                attention::Key::Open(agent) => self.open_question(&agent, now),
                attention::Key::Leave => self.overlay = Overlay::None,
                attention::Key::None => {}
            },
            Overlay::None if self.focus == Focus::Sidebar => match self.sidebar.key(key) {
                sidebar::Key::Open(agent) => self.open_picked(&agent, now),
                sidebar::Key::Leave => {
                    if self.place.take().is_some() {
                        self.status = self.hint();
                    }
                    if self.layout.count() > 0 {
                        self.focus = Focus::View;
                    }
                }
                sidebar::Key::None => {}
            },
            Overlay::None => {
                if let Some(view) = self.active_view() {
                    view.event(&Event::Key(*key));
                }
            }
        }
    }

    pub(super) fn action(&mut self, action: Action, armed: Option<Armed>, now: Instant) {
        let prefix = self.prefix();
        match action {
            Action::Sidebar => {
                (self.focus, self.overlay, self.place) = (Focus::Sidebar, Overlay::None, None);
            }
            Action::NewTab => {
                (self.focus, self.overlay) = (Focus::Sidebar, Overlay::None);
                self.place = Some(Place::Tab);
                self.status = String::from("pick an agent for a new tab, then Enter");
            }
            Action::Split(split) => {
                let full = self
                    .layout
                    .tabs
                    .get(self.layout.current)
                    .is_some_and(|tab| tab.views.len() > 1);
                if full {
                    self.status = String::from("this tab already shows two views; close one first");
                } else {
                    (self.focus, self.overlay) = (Focus::Sidebar, Overlay::None);
                    self.place = Some(Place::Split(split));
                    self.status = String::from("pick an agent for the split, then Enter");
                }
            }
            Action::Close => {
                let Some(active) = self.layout.active().map(str::to_owned) else {
                    return;
                };
                let busy = self.views.get(&active).is_some_and(View::busy);
                if busy && armed != Some(Armed::Close) {
                    self.armed = Some(Armed::Close);
                    self.status =
                        format!("a write is in flight; {prefix} w again closes the view anyway");
                } else {
                    self.close(&active, Some(now));
                    self.status = format!(
                        "closed {}; the agent keeps running",
                        sidebar::short(&active)
                    );
                }
            }
            Action::Tab(index) => self.show(|layout| layout.select(index)),
            Action::Next => self.show(Layout::next),
            Action::Previous => self.show(Layout::previous),
            Action::Other => self.show(Layout::other),
            Action::Attention => {
                self.overlay = Overlay::Attention;
                self.attention.selected = 0;
            }
            Action::Spawn => {
                let directory = std::env::current_dir()
                    .map(|directory| directory.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut dialog = spawn::Dialog::new(directory, self.args.root.is_some());
                dialog.lists = Arc::clone(&self.lists);
                self.overlay = Overlay::Spawn(Box::new(dialog));
                // The worker ends only with the process; a send cannot fail first.
                let _ = self.asks.send(Ask::Roles);
            }
            Action::Quit => self.quit(armed),
        }
    }

    pub(super) fn show(&mut self, change: impl FnOnce(&mut Layout)) {
        change(&mut self.layout);
        self.overlay = Overlay::None;
        if self.layout.count() > 0 {
            self.focus = Focus::View;
        }
    }

    pub(super) fn quit(&mut self, armed: Option<Armed>) {
        if self.views.values().any(View::busy) && armed != Some(Armed::Quit) {
            self.armed = Some(Armed::Quit);
            self.status = String::from("a write is still in flight; Ctrl-C again to quit anyway");
        } else {
            self.quit = true;
        }
    }
}
