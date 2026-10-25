//! Full-screen commands end once their terminal hangs up. crossterm's event
//! reader never returns after its terminal reaches end of input, so a closed
//! window, a killed multiplexer session or a dropped connection would otherwise
//! leave the process spinning, deaf to signals.
use std::{os::fd::AsFd, time::Duration};

/// How often the terminal is checked.
const CHECK: Duration = Duration::from_millis(250);

/// Watch stdin's terminal on a detached thread and end the process once it has
/// hung up. Nothing can be shown or restored on a terminal that is gone.
pub(super) fn watch() {
    std::thread::spawn(|| {
        while !crate::os::terminal::hung_up(std::io::stdin().as_fd()) {
            std::thread::sleep(CHECK);
        }
        std::process::exit(1);
    });
}
