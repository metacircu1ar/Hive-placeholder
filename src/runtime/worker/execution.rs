//! Borrow the immutable worker identity used by every execution-fenced hub operation.
use super::{Agent, Core, Result, Shared};
use std::future::Future;

#[derive(Clone, Copy)]
pub(super) struct ExecutionHandle<'a> {
    pub shared: &'a Shared,
    pub id: &'a str,
    pub execution: &'a str,
}

impl<'a> ExecutionHandle<'a> {
    pub(crate) fn new(shared: &'a Shared, id: &'a str, execution: &'a str) -> Self {
        Self {
            shared,
            id,
            execution,
        }
    }

    // Forward synchronously so the writer sees the operation's original call site.
    // Keep its live execution check and transaction boundary on every invocation.
    #[track_caller]
    pub(crate) fn agent<T: Send + 'static>(
        self,
        operation: impl FnOnce(&mut Core, &mut Agent) -> Result<T> + Send + 'static,
    ) -> impl Future<Output = Result<T>> + Send + 'a {
        let id = self.id.to_owned();
        let execution = self.execution.to_owned();
        self.shared.with(move |core| {
            let mut agent = core.store().agent(&id)?;
            if agent.execution_id.as_deref() != Some(execution.as_str()) {
                return Err(crate::error::Error::stale_execution(
                    "Execution no longer owns this agent.",
                ));
            }
            operation(core, &mut agent)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::RuntimeFixture;

    #[tokio::test]
    async fn a_retained_handle_rechecks_the_live_execution_before_every_operation() {
        let fixture = RuntimeFixture::new();
        let execution = ExecutionHandle::new(&fixture.hub, "agent", "execution");
        assert_eq!(
            execution
                .agent(|_, agent| Ok(agent.id.clone()))
                .await
                .unwrap(),
            "agent"
        );
        fixture
            .hub
            .with(|core| {
                let mut agent = core.store().agent("agent")?;
                agent.execution_id = Some("replacement".into());
                core.store().save(&agent)
            })
            .await
            .unwrap();
        let error = execution
            .agent::<()>(|_, _| panic!("a stale execution must never enter its operation"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "stale_execution");
        assert_eq!(error.message(), "Execution no longer owns this agent.");
        assert_eq!(
            ExecutionHandle::new(&fixture.hub, "agent", "replacement")
                .agent(|_, agent| Ok(agent.execution_id.clone()))
                .await
                .unwrap()
                .as_deref(),
            Some("replacement")
        );
        fixture.finish();
    }
}
