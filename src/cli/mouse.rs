//! Click reporting for full-screen views: presses, releases and wheel steps in
//! SGR coordinates (`?1000`, `?1006`). Crossterm's `EnableMouseCapture` also
//! turns on any-motion tracking, which would flood the loop with move events.
use ratatui::crossterm::{Command, event::DisableBracketedPaste, execute};
use std::fmt;

/// Turn click reporting on (`true`) or off; off reverses the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Reporting(pub bool);

impl Command for Reporting {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(if self.0 {
            "\u{1b}[?1000h\u{1b}[?1006h"
        } else {
            "\u{1b}[?1006l\u{1b}[?1000l"
        })
    }
}

/// On a panic, turn click reporting and bracketed paste off before the
/// existing hook (ratatui's) restores the screen, so a crashed view never
/// leaves the terminal reporting the mouse.
pub(super) fn restore_on_panic() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The terminal may be gone; the original hook still runs.
        let _ = execute!(std::io::stdout(), Reporting(false), DisableBracketedPaste);
        previous(info);
    }));
}

#[cfg(test)]
mod tests;
