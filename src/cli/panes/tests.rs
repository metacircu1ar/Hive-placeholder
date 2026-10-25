use super::*;

#[test]
fn the_backend_follows_the_terminal_unless_chosen() {
    assert_eq!(kind(None, Some(Kind::Zellij), true).unwrap(), Kind::Zellij);
    // The multiplexer comes before the terminal it runs in.
    assert_eq!(kind(None, Some(Kind::Tmux), true).unwrap(), Kind::Tmux);
    assert_eq!(
        kind(Some(Kind::Zellij), Some(Kind::Tmux), false).unwrap(),
        Kind::Zellij
    );
    assert_eq!(kind(Some(Kind::Tmux), None, false).unwrap(), Kind::Tmux);
    let none = kind(None, None, false).unwrap_err();
    assert_eq!(none.code(), "pane_backend_unavailable");
    assert!(none.message().contains("hivectl tui"), "{none}");
    if cfg!(target_os = "macos") {
        assert_eq!(kind(None, None, true).unwrap(), Kind::Ghostty);
        assert_eq!(
            kind(Some(Kind::Ghostty), None, false).unwrap(),
            Kind::Ghostty
        );
    } else {
        assert_eq!(
            kind(None, None, true).unwrap_err().code(),
            "pane_backend_unavailable"
        );
        assert_eq!(
            kind(Some(Kind::Ghostty), Some(Kind::Tmux), false)
                .unwrap_err()
                .code(),
            "pane_backend_unavailable"
        );
    }
}

#[test]
fn a_nested_multiplexer_is_chosen_by_who_owns_the_terminal() {
    let never = || -> bool { panic!("checked only when both are set") };
    assert_eq!(multiplexer(false, false, never), None);
    assert_eq!(multiplexer(true, false, never), Some(Kind::Zellij));
    assert_eq!(multiplexer(false, true, never), Some(Kind::Tmux));
    assert_eq!(multiplexer(true, true, || true), Some(Kind::Tmux));
    // A failed or negative check means zellij, the live multiplexer.
    assert_eq!(multiplexer(true, true, || false), Some(Kind::Zellij));
}
