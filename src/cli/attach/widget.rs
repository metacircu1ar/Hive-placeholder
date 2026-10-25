//! One agent's view as a widget: its follower, command worker and app, drawn
//! into any area. `hivectl attach` shows one on the whole screen; `hivectl tui` many.
use super::{
    app::{App, Exit, Mode},
    client::{self, Command, Update},
};
use ratatui::{Frame, crossterm::event::Event, layout::Rect};
use std::{
    path::Path,
    sync::mpsc::{Receiver, Sender},
    time::Instant,
};

pub(in crate::cli) struct View {
    _subscription: Option<client::Subscription>,
    app: App,
    updates: Receiver<Update>,
    commands: Sender<Command>,
}

impl View {
    /// Start following `target`; the first frame shows "Connecting".
    pub(in crate::cli) fn open(root: &Path, target: &str, mode: Mode) -> Self {
        let (updates, commands, subscription) =
            client::start(root.to_path_buf(), target.to_owned());
        Self {
            _subscription: Some(subscription),
            app: App::new(target.to_owned(), mode),
            updates,
            commands,
        }
    }

    /// A view showing `snapshot`, without a daemon: its updates never arrive
    /// and its commands go nowhere.
    #[cfg(test)]
    pub(in crate::cli) fn detached(target: &str, mode: Mode, snapshot: &serde_json::Value) -> Self {
        let (_, updates) = std::sync::mpsc::sync_channel(1);
        let (commands, _) = std::sync::mpsc::channel();
        let mut app = App::new(target.to_owned(), mode);
        app.update(Update::Snapshot(snapshot.clone()));
        Self {
            _subscription: None,
            app,
            updates,
            commands,
        }
    }

    /// Deliver the daemon's refusal of the first attach, as the follower would.
    #[cfg(test)]
    pub(in crate::cli) fn refuse(&mut self, reply: serde_json::Value) {
        self.app.update(Update::Refused(reply));
    }

    /// Apply every queued update; `true` when any arrived.
    pub(in crate::cli) fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.updates.try_recv() {
            self.app.update(update);
            changed = true;
        }
        self.flush();
        changed
    }

    fn flush(&mut self) {
        for command in self.app.commands.drain(..) {
            // The worker ends only with the process; a send cannot fail first.
            let _ = self.commands.send(command);
        }
    }

    /// `false` when the event changed nothing, so no frame is needed.
    pub(in crate::cli) fn event(&mut self, event: &Event) -> bool {
        let changed = self.app.event(event);
        self.flush();
        changed
    }

    pub(in crate::cli) fn tick(&mut self, now: Instant) {
        self.app.tick(now);
    }

    /// Text the view asked to copy to the terminal's clipboard, once.
    pub(in crate::cli) fn take_clipboard(&mut self) -> Option<String> {
        self.app.take_clipboard()
    }

    pub(super) fn agent_id(&self) -> Option<&str> {
        self.app
            .model
            .as_ref()
            .map(|model| model.header.id.as_str())
    }

    /// Why the view wants to close or switch, once.
    pub(in crate::cli) fn take_exit(&mut self) -> Option<Exit> {
        self.app.exit.take()
    }

    pub(in crate::cli) fn draw(&mut self, frame: &mut Frame<'_>, area: Rect, active: bool) {
        self.app.draw_into(frame, area, active);
    }

    pub(in crate::cli) fn busy(&self) -> bool {
        self.app.busy()
    }

    pub(in crate::cli) fn waiting(&self) -> bool {
        self.app.waiting()
    }

    pub(in crate::cli) fn set_hint(&mut self, hint: String) {
        self.app.set_hint(hint);
    }

    pub(in crate::cli) fn set_status(&mut self, status: String) {
        self.app.status = status;
    }

    pub(in crate::cli) fn focus_card(&mut self) {
        self.app.focus_card();
    }

    /// The agent's outcome once its view has seen it end.
    pub(in crate::cli) fn outcome(&self) -> Option<&str> {
        let header = &self.app.model.as_ref()?.header;
        header.ended.then_some(header.outcome.as_str())
    }
}
