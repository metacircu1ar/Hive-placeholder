use super::*;
use std::{cell::Cell, collections::BTreeMap, rc::Rc};

#[test]
fn state_publication_retains_io_classification_and_complete_cleanup_evidence() {
    let error = Error::state_publication(
        atomic_fs::Error::new(std::io::Error::from_raw_os_error(libc::EIO), true)
            .with_cleanup_error(std::io::Error::from_raw_os_error(libc::EPERM))
            .with_retained("private-preparation"),
    );
    for error in [&error, &error.clone()] {
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert_eq!(
            error.io_kind(),
            Some(std::io::Error::from_raw_os_error(libc::EIO).kind())
        );
        let evidence = error
            .source()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .unwrap();
        assert!(evidence.published());
        assert_eq!(
            evidence.cleanup_error().as_ref().unwrap().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            evidence.retained(),
            Some(std::path::Path::new("private-preparation"))
        );
    }
    let wrapped = Error::provider_io("wrapper").with_source(error);
    assert_eq!(wrapped.io_kind(), None);
    assert_eq!(wrapped.io_raw_os_error(), None);
}

#[test]
fn immediate_source_metadata_preserves_io_and_submission_boundaries() {
    let raw = rustix::io::Errno::PIPE.raw_os_error();
    let error = Error::state_io_from(std::io::Error::from_raw_os_error(raw));
    assert_eq!(error.io_kind(), Some(std::io::ErrorKind::BrokenPipe));
    assert_eq!(error.io_raw_os_error(), Some(raw));
    assert!(!error.input_was_not_submitted());
    let wrapped = Error::provider_io("wrapped").with_source(error.clone());
    assert_eq!(wrapped.io_kind(), None);
    assert_eq!(wrapped.io_raw_os_error(), None);
    let before = error.before_submission();
    assert!(before.input_was_not_submitted());
    assert!(before.clone().input_was_not_submitted());
    assert_eq!(before.io_kind(), None);
    assert_eq!(before.io_raw_os_error(), None);
    let original = before.source().unwrap().source().unwrap();
    assert_eq!(
        original.downcast_ref::<Error>().unwrap().io_raw_os_error(),
        Some(raw)
    );
    let replaced = before.with_source(std::io::Error::from_raw_os_error(raw));
    assert!(!replaced.input_was_not_submitted());
    assert_eq!(replaced.io_raw_os_error(), Some(raw));
}

#[test]
fn public_errors_serialize_codes_and_messages_without_private_causes() {
    // Exercise each current constructor and keep private causes out of JSON.
    let messages: BTreeMap<&'static str, Vec<&'static str>> =
        serde_json::from_str(include_str!("public_messages.json")).unwrap();
    assert_eq!(messages.len(), CONSTRUCTORS.len());
    for &(code, constructor) in CONSTRUCTORS {
        for &message in messages.get(code).unwrap() {
            let expected = format!(
                "{{\"code\":{},\"message\":{}}}",
                serde_json::to_string(code).unwrap(),
                serde_json::to_string(message).unwrap()
            );
            for error in [
                constructor(message),
                constructor(message).with_source(std::io::Error::other("secret-canary")),
            ] {
                assert_eq!(error.code(), code);
                assert_eq!(error.message(), message);
                assert_eq!(serde_json::to_string(&error).unwrap(), expected);
                assert_eq!(error.to_string(), format!("{code}: {message}"));
                assert_eq!(
                    format!("{error:?}"),
                    format!("Error {{ code: {code:?}, message: {message:?} }}")
                );
                assert!(!format!("{error:#?}").contains("secret-canary"));
            }
        }
    }
}

#[test]
fn cloned_errors_preserve_original_causes_and_nested_source_chains() {
    let error = Error::state_io_from(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "secret-canary",
    ));
    let cloned = error.clone();
    let original = error
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
    let retained = cloned
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
    assert!(std::ptr::eq(original, retained));
    assert_eq!(retained.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(retained.to_string(), "secret-canary");

    let wrapped =
        Error::daemon_busy("A daemon or its surviving provider owns this state directory.")
            .with_source(cloned);
    assert!(matches!(
        wrapped.source().unwrap().downcast_ref::<Error>().unwrap(),
        Error::StateIo { .. }
    ));
    let root = wrapped.source().unwrap().source().unwrap();
    assert_eq!(
        root.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(!wrapped.to_string().contains("secret-canary"));
    assert!(!format!("{wrapped:#?}").contains("secret-canary"));
    assert!(
        !serde_json::to_string(&wrapped)
            .unwrap()
            .contains("secret-canary")
    );
}

struct DropNotice(Rc<Cell<usize>>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn channel_error_sources_release_payloads_and_preserve_full_or_closed() {
    use tokio::sync::mpsc::error::{SendError, TrySendError};

    let drops = Rc::new(Cell::new(0));
    let error = Error::daemon_stopping("Runtime store is unavailable.")
        .with_send_source(SendError(DropNotice(drops.clone())));
    assert_eq!(drops.get(), 1);
    assert!(error.source().unwrap().is::<SendError<()>>());

    for (index, full) in [true, false].into_iter().enumerate() {
        let payload = DropNotice(drops.clone());
        let cause = if full {
            TrySendError::Full(payload)
        } else {
            TrySendError::Closed(payload)
        };
        let error = Error::output_limit("Agent event queue is not being drained.")
            .with_try_send_source(cause);
        assert_eq!(drops.get(), index + 2);
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<TrySendError<()>>()
            .unwrap();
        assert_eq!(matches!(source, TrySendError::Full(())), full);
        assert_eq!(matches!(source, TrySendError::Closed(())), !full);
        assert_eq!(
            serde_json::to_string(&error.clone()).unwrap(),
            serde_json::to_string(&error).unwrap()
        );
    }
}

#[test]
fn configuration_io_failure_retains_cause_without_disclosing_the_path() {
    let temp = tempfile::tempdir().unwrap();
    let error = crate::config::Config::load(&temp.path().join("secret-canary")).unwrap_err();
    assert!(matches!(error, Error::ConfigUnavailable { .. }));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        r#"{"code":"config_unavailable","message":"Cannot read the configuration file."}"#
    );
    assert!(!format!("{error} {error:?}").contains("secret-canary"));
}

#[test]
fn semantic_configuration_and_lock_errors_retain_their_causes() {
    let error = crate::config::Config::parse("auth_profiles: [secret-canary").unwrap_err();
    assert!(matches!(error, Error::ConfigInvalid { .. }));
    assert!(error.source().is_some());
    assert!(!format!("{error} {error:#?}").contains("secret-canary"));
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("secret-canary")
    );

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("secret-canary.lock");
    let _exclusive = crate::state::Lock::acquire(&path).unwrap();
    let error = crate::state::Lock::shared(&path).err().unwrap();
    assert!(matches!(error, Error::ProfileBusy { .. }));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        r#"{"code":"profile_busy","message":"An exclusive operation owns this profile."}"#
    );
}

#[test]
fn typed_codes_preserve_wire_strings_and_reject_unknown_current_errors() {
    for (code, make) in CONSTRUCTORS {
        let kind = make("message").kind();
        assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{code}\""));
        assert_eq!(
            serde_json::from_str::<ErrorCode>(&format!("\"{code}\"")).unwrap(),
            kind
        );
    }
    for raw in [r#""future_error""#, r#"{"agent_busy":null}"#, "null", "7"] {
        assert!(serde_json::from_str::<ErrorCode>(raw).is_err());
    }
    assert!(Error::agent_busy("busy").is_retryable_restore());
    assert!(!Error::authentication_required("auth").is_retryable_restore());
}

#[test]
fn input_errors_retain_causes_but_never_serialize_received_input() {
    use hive_input::{Code, InputError};
    let input = InputError::new(
        Code::InvalidPath,
        "Invalid path.",
        "/path",
        "a file",
        vec!["hivectl read AGENT_ID".into()],
    )
    .unwrap()
    .with_public_received("private-path-canary");
    let error = Error::from(input).with_source(std::io::Error::other("private-source-canary"));
    assert_eq!(error.code(), "invalid_path");
    assert!(std::error::Error::source(&error).is_some());
    let json = serde_json::to_value(&error).unwrap();
    assert_eq!(json["field"], "/path");
    assert_eq!(json.as_object().unwrap().len(), 5);
    for text in [
        serde_json::to_string(&error).unwrap(),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(!text.contains("private-"));
    }
    let observation = crate::calls::ObservationError::from(error);
    assert_eq!(serde_json::to_value(&observation).unwrap(), json);
    let operational = Error::invalid_path("Invalid path.");
    assert_eq!(
        serde_json::to_value(operational)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        2
    );
}
