//! Startup for this component.
use super::{
    Backend, Error, Failure, GHOSTTY_READY_POLL, GHOSTTY_READY_TIMEOUT, Instant, Path, Result,
    Zellij,
};

/// The zellij backend for this session, keyed for its lock.
pub(super) fn zellij() -> Result<(Box<dyn Backend>, String)> {
    use sha2::Digest;
    let session = std::env::var("ZELLIJ_SESSION_NAME")
        .ok()
        .filter(|_| std::env::var_os("ZELLIJ").is_some())
        .ok_or_else(|| {
            Error::pane_backend_unavailable(
                "hivectl panes runs inside a zellij session; start zellij, then run it in a pane.",
            )
        })?;
    // Without the executable every pane command would fail: refuse at once.
    let mut probe = std::process::Command::new("zellij");
    probe.arg("--version");
    if crate::os::output(&mut probe).is_err() {
        return Err(Error::pane_backend_unavailable(
            "zellij was not found on PATH; install it or add it to PATH.",
        ));
    }
    let digest = sha2::Sha256::digest(session.as_bytes());
    Ok((Box::new(Zellij), format!("{digest:x}")))
}

/// The controller's tmux pane, when it runs inside tmux.
pub(super) fn tmux_pane() -> Option<String> {
    std::env::var_os("TMUX")?;
    std::env::var("TMUX_PANE")
        .ok()
        .filter(|pane| !pane.is_empty())
}

/// Whether stdin is this tmux pane's terminal: the same device number.
pub(super) fn tmux_owns_terminal(pane: &str) -> bool {
    let Some(tty) = super::tmux::pane_tty(pane) else {
        return false;
    };
    match (
        rustix::fs::stat(tty.as_str()),
        rustix::fs::fstat(std::io::stdin()),
    ) {
        (Ok(pane), Ok(stdin)) => pane.st_rdev == stdin.st_rdev,
        _ => false,
    }
}

/// The tmux backend for this session, keyed for its lock: one controller per
/// tmux server socket and session.
pub(super) fn tmux(root: &Path) -> Result<(Box<dyn Backend>, String)> {
    use sha2::Digest;
    let pane = tmux_pane().ok_or_else(|| {
        Error::pane_backend_unavailable(
            "hivectl panes --backend tmux runs inside a tmux session; start tmux, then \
                run it in a pane.",
        )
    })?;
    // Without the executable every pane command would fail: refuse at once.
    let mut probe = std::process::Command::new("tmux");
    probe.arg("-V");
    if crate::os::output(&mut probe).is_err() {
        return Err(Error::pane_backend_unavailable(
            "tmux was not found on PATH; install it or add it to PATH.",
        ));
    }
    let (session, socket) = super::tmux::session(&pane).ok_or_else(|| {
        Error::pane_backend_unavailable(
            "tmux did not answer; is this pane's tmux server still running?",
        )
    })?;
    let digest = sha2::Sha256::digest(format!("{socket}\0{session}").as_bytes());
    let root = root.to_string_lossy().into_owned();
    Ok((
        Box::new(super::tmux::Tmux::new(session, pane, root)),
        format!("tmux-{digest:x}"),
    ))
}

/// Find the controller's own Ghostty terminal and check that titles reach
/// Ghostty, before the terminal enters raw mode.
pub(super) fn ghostty() -> Result<super::ghostty::Ghostty> {
    use ratatui::crossterm::{execute, terminal::SetTitle};
    use std::io::Write;
    // Scripting another terminal's Ghostty would launch it: refuse instead.
    if !std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "ghostty") {
        return Err(Error::pane_backend_unavailable(
            "Ghostty panes must be run from a Ghostty terminal.",
        ));
    }
    // Ghostty reports no tty: a unique title finds the controller's terminal.
    let title = format!(
        "{}{}",
        super::ghostty::CONTROLLER_TITLE,
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let mut stdout = std::io::stdout();
    // The first script waits for the macOS Automation prompt, if one is shown.
    writeln!(
        stdout,
        "Checking Ghostty; answer the macOS Automation prompt if one appears."
    )
    .and_then(|()| execute!(stdout, SetTitle(&title)))
    .map_err(Error::state_io_from)?;
    let mut backend = super::ghostty::Ghostty::new(title.clone());
    let deadline = Instant::now() + GHOSTTY_READY_TIMEOUT;
    loop {
        let listing = backend.list().map_err(|failure| match failure {
            Failure::Fatal(error) => error,
            Failure::Retry(_) => Error::pane_backend_unavailable(
                "Ghostty did not answer its scripting request; is Ghostty 1.3 or newer running?",
            ),
        })?;
        if listing.iter().any(|terminal| terminal.name == title) {
            return Ok(backend);
        }
        if Instant::now() >= deadline {
            return Err(Error::pane_backend_unavailable(
                "Ghostty did not report this terminal's title; remove Ghostty's title \
                    option to use Ghostty panes.",
            ));
        }
        std::thread::sleep(GHOSTTY_READY_POLL);
    }
}
