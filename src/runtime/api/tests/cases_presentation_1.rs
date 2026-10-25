//! Presentation regressions.
use super::*;

#[test]
fn live_viewing_is_human_only_and_never_runs_in_the_writer() {
    use crate::runtime::{core::preflight::Facts, hub::Caller};
    let tmp = crate::runtime::test_directory();
    let mut core = core(tmp.path());
    for raw in [
        json!({"op":"live","target":"agent","after":{"epoch":1,"generation":1,"seq":0,"durable":0}}),
        json!({"op":"tree"}),
        json!({"op":"attach","target":"agent"}),
    ] {
        let request = RequestEnvelope::new(raw.clone());
        assert!(
            request.request.caller_policy(&Caller::Human).is_ok(),
            "{raw}"
        );
        for caller in [Caller::test_agent("agent"), Caller::System] {
            assert!(matches!(
                request.request.caller_policy(&caller),
                Err(Error::UnsupportedOperation { .. })
            ));
        }
        // The control server answers these itself; a misrouted one is refused.
        assert!(
            matches!(
                core.dispatch(&Caller::Human, "live", &request, &Facts::default()),
                Err(Error::UnsupportedOperation { .. })
            ),
            "{raw}"
        );
    }
}
