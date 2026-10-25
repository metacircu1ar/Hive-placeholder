//! Public error bytes and direct causal metadata at the provider boundary.
use super::*;
use std::error::Error as _;

#[test]
fn protocol_classifications_keep_exact_public_bytes() -> serde_json::Result<()> {
    let cases = [
        (
            protocol(),
            r#"{"code":"provider_protocol_error","message":"Invalid provider protocol state."}"#,
        ),
        (
            transport_protocol(),
            r#"{"code":"provider_protocol_error","message":"Cannot exchange a valid provider protocol message."}"#,
        ),
        (
            native_protocol(),
            r#"{"code":"provider_protocol_error","message":"Invalid native Claude protocol state."}"#,
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(serde_json::to_string(&error)?, expected);
        assert!(error.source().is_none());
    }
    Ok(())
}

#[test]
fn a_concrete_exchange_failure_keeps_its_direct_source() -> serde_json::Result<()> {
    let error = transport_protocol().with_source(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "native input closed",
    ));
    assert_eq!(
        error
            .source()
            .and_then(|cause| cause.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::BrokenPipe),
    );
    assert_eq!(
        serde_json::to_string(&error)?,
        serde_json::to_string(&transport_protocol())?
    );
    Ok(())
}

#[test]
fn unavailable_keeps_its_uncertain_delivery_contract() -> serde_json::Result<()> {
    let error = unavailable();
    assert!(error.source().is_none());
    assert!(!error.input_was_not_submitted());
    assert_eq!(
        serde_json::to_string(&error)?,
        r#"{"code":"provider_unavailable","message":"Provider connection is unavailable; uncertain input is not retried."}"#,
    );
    Ok(())
}
