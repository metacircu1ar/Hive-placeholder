use super::*;

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn control(character: char) -> KeyEvent {
    key(KeyCode::Char(character), KeyModifiers::CONTROL)
}

fn bracket() -> Prefix {
    Prefix::parse("ctrl-]").unwrap()
}

#[test]
fn the_default_prefix_is_ctrl_a() {
    let prefix = Prefix::default();
    assert_eq!(Prefix::parse("ctrl-a"), Some(prefix));
    assert!(prefix.matches(&control('a')));
    assert!(!prefix.matches(&key(KeyCode::Char('a'), KeyModifiers::NONE)));
    assert_eq!(prefix.label(), "Ctrl-A");
}

#[test]
fn a_bracket_prefix_matches_under_both_encodings() {
    let prefix = bracket();
    // The legacy encoding reports byte 0x1D as Ctrl-5; kitty reports Ctrl-].
    assert!(prefix.matches(&control('5')));
    assert!(prefix.matches(&control(']')));
    assert!(!prefix.matches(&key(KeyCode::Char(']'), KeyModifiers::NONE)));
    assert!(!prefix.matches(&control('c')));
    assert_eq!(prefix.label(), "Ctrl-]");
}

#[test]
fn configured_prefixes_are_normalised_and_unusable_ones_refused() {
    assert_eq!(Prefix::parse("Ctrl-5"), Some(bracket()));
    let backslash = Prefix::parse("ctrl-\\").unwrap();
    assert!(backslash.matches(&control('4')) && backslash.matches(&control('\\')));
    let letter = Prefix::parse("ctrl-a").unwrap();
    assert!(letter.matches(&control('a')) && !letter.matches(&control('5')));
    assert!(letter.matches(&key(
        KeyCode::Char('A'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT
    )));
    for refused in [
        "ctrl-i", "ctrl-m", "ctrl-[", "ctrl-h", "ctrl-c", "ctrl-r", "ctrl-j", "alt-x", "ctrl-",
        "ctrl-ab", "ctrl-1", "x",
    ] {
        assert_eq!(Prefix::parse(refused), None, "{refused}");
    }
}

#[test]
fn config_files_default_parse_and_refuse_with_named_fields() {
    assert_eq!(parse(b"").unwrap(), Settings::default());
    assert_eq!(
        parse(b"prefix = \"ctrl-b\"\n").unwrap().prefix,
        Prefix::parse("ctrl-b").unwrap()
    );
    let error = parse(b"prefix = \"ctrl-c\"\n").unwrap_err();
    assert_eq!(error.code(), "config_invalid");
    assert!(error.to_string().contains("prefix"), "{error}");
    for invalid in [
        &b"prefix = 1\n"[..],
        b"other = \"x\"\n",
        b"\xff",
        b"prefix =",
        b"mouse = \"no\"\n",
    ] {
        assert_eq!(parse(invalid).unwrap_err().code(), "config_invalid");
    }
    let oversized = vec![b'#'; crate::config::CONFIG_BYTES + 1];
    assert_eq!(parse(&oversized).unwrap_err().code(), "config_invalid");
    let directory = crate::runtime::test_directory();
    assert_eq!(load(directory.path()).unwrap(), Settings::default());
    std::fs::write(directory.path().join("tui.toml"), "prefix = \"ctrl-g\"\n").unwrap();
    assert_eq!(
        load(directory.path()).unwrap().prefix,
        Prefix::parse("ctrl-g").unwrap()
    );
    // A directory in place of the file cannot be read.
    let unreadable = crate::runtime::test_directory();
    std::fs::create_dir(unreadable.path().join("tui.toml")).unwrap();
    assert_eq!(
        load(unreadable.path()).unwrap_err().code(),
        "config_unavailable"
    );
}

#[test]
fn the_key_after_the_prefix_is_an_action_or_dropped() {
    let mut router = Router::new(bracket());
    let start = Instant::now();
    let plain = |character| key(KeyCode::Char(character), KeyModifiers::NONE);
    assert_eq!(router.route(&plain('x'), start), Routed::Key);
    assert_eq!(router.route(&control('5'), start), Routed::Pending);
    assert!(router.pending());
    assert_eq!(
        router.route(&plain('v'), start),
        Routed::Action(Action::Split(super::super::layout::Split::Columns))
    );
    assert!(!router.pending());
    for (character, action) in [
        ('s', Action::Sidebar),
        ('t', Action::NewTab),
        ('h', Action::Split(super::super::layout::Split::Rows)),
        ('w', Action::Close),
        ('1', Action::Tab(0)),
        ('9', Action::Tab(8)),
        ('n', Action::Next),
        ('p', Action::Previous),
        ('o', Action::Other),
        ('a', Action::Attention),
        ('c', Action::Spawn),
        ('q', Action::Quit),
    ] {
        router.route(&control(']'), start);
        assert_eq!(
            router.route(&plain(character), start),
            Routed::Action(action)
        );
    }
    // An unbound key is dropped, never typed into the view.
    router.route(&control('5'), start);
    assert_eq!(router.route(&plain('z'), start), Routed::Dropped);
    router.route(&control('5'), start);
    assert_eq!(
        router.route(&key(KeyCode::Enter, KeyModifiers::NONE), start),
        Routed::Dropped
    );
    // The prefix twice passes the prefix key on.
    router.route(&control('5'), start);
    assert_eq!(router.route(&control('5'), start), Routed::Prefix);
    assert_eq!(router.route(&plain('s'), start), Routed::Key);
}

#[test]
fn a_pending_prefix_expires() {
    let mut router = Router::new(bracket());
    let start = Instant::now();
    router.route(&control('5'), start);
    assert!(!router.expire(start + PREFIX_TIMEOUT));
    assert!(router.expire(start + PREFIX_TIMEOUT + Duration::from_millis(1)));
    assert!(!router.pending());
    // After expiry the key goes to the view again.
    router.route(&control('5'), start);
    let late = start + PREFIX_TIMEOUT + Duration::from_millis(1);
    assert_eq!(
        router.route(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE), late),
        Routed::Key
    );
}

#[test]
fn the_mouse_setting_defaults_on_and_turns_off() {
    assert!(parse(b"").unwrap().mouse);
    assert!(!parse(b"mouse = false\n").unwrap().mouse);
    let both = parse(b"prefix = \"ctrl-b\"\nmouse = true\n").unwrap();
    assert_eq!(both.prefix, Prefix::parse("ctrl-b").unwrap());
    assert!(both.mouse);
}

#[test]
fn provider_tables_list_models_and_efforts_and_refuse_bad_names() {
    let settings = parse(
        b"[codex]\nmodels = [\"gpt-6-luna\", \"gpt-6\"]\nefforts = [\"low\", \"high\"]\n\n[claude]\nmodels = [\"opus\"]\n",
    )
    .unwrap();
    assert_eq!(settings.lists.codex.models, ["gpt-6-luna", "gpt-6"]);
    assert_eq!(settings.lists.codex.efforts, ["low", "high"]);
    assert_eq!(settings.lists.claude.models, ["opus"]);
    assert!(settings.lists.claude.efforts.is_empty());
    assert_eq!(parse(b"[codex]\n").unwrap().lists, Lists::default());
    let refused = |data: &[u8], word: &str| {
        let error = parse(data).unwrap_err();
        assert_eq!(error.code(), "config_invalid", "{error}");
        assert!(error.to_string().contains(word), "{error}");
    };
    refused(b"[codex]\nmodels = [\"bad name\"]\n", "[codex] models");
    refused(b"[codex]\nmodels = [\"-x\"]\n", "[codex] models");
    refused(
        b"[claude]\nmodels = [\"opus\", \"opus\"]\n",
        "[claude] models",
    );
    refused(b"[codex]\nefforts = [\"fast\"]\n", "[codex] efforts");
    refused(
        b"[claude]\nefforts = [\"low\", \"low\"]\n",
        "[claude] efforts",
    );
    // Messages never echo the refused value.
    let error = parse(b"[codex]\nmodels = [\"secret value\"]\n").unwrap_err();
    assert!(!error.to_string().contains("secret"), "{error}");
    let many: Vec<String> = (0..=MAX_LISTED)
        .map(|index| format!("\"m{index}\""))
        .collect();
    refused(
        format!("[codex]\nmodels = [{}]\n", many.join(", ")).as_bytes(),
        "[codex] models",
    );
    let most: Vec<String> = (0..MAX_LISTED)
        .map(|index| format!("\"m{index}\""))
        .collect();
    assert_eq!(
        parse(format!("[codex]\nmodels = [{}]\n", most.join(", ")).as_bytes())
            .unwrap()
            .lists
            .codex
            .models
            .len(),
        MAX_LISTED
    );
    // Unknown tables and keys, and lists of other types, are refused.
    for invalid in [
        &b"[gemini]\nmodels = [\"x\"]\n"[..],
        b"[codex]\nmodel = [\"x\"]\n",
        b"[codex]\nmodels = \"x\"\n",
        b"[codex]\nmodels = [1]\n",
    ] {
        refused(invalid, "tui.toml");
    }
    // The efforts messages name every accepted value.
    for effort in hive_input::validate::EFFORTS {
        assert!(INVALID_CODEX_EFFORTS.contains(effort), "{effort}");
        assert!(INVALID_CLAUDE_EFFORTS.contains(effort), "{effort}");
    }
    // Provider tables alone keep the other defaults.
    let only = parse(b"[claude]\nmodels = [\"opus\"]\n").unwrap();
    assert_eq!(
        (only.prefix, only.mouse),
        (Settings::default().prefix, true)
    );
}
