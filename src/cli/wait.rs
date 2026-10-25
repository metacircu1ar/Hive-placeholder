//! Bounded event and task observation from the CLI.
use super::{
    AgentState, CommandOutcome, Error, Input, Instant, MAX_WAIT_SECONDS, MIN_TIMEOUT_SECONDS,
    ReadArgs, Request, Result, StatusArgs, TaskOutcome, WAIT_EVENT_PAGE_ROWS, WAIT_POLL_INTERVAL,
    api, runtime,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(super) enum WaitUntil {
    Completed,
    TurnFinished,
}

pub(super) struct Waiter<'a> {
    pub(super) root: &'a std::path::Path,
    pub(super) target: String,
    pub(super) until: WaitUntil,
    pub(super) after: u64,
    pub(super) deadline: Instant,
}
impl Waiter<'_> {
    pub(super) fn run(mut self) -> Result<CommandOutcome> {
        use uuid::Uuid;
        loop {
            let request = Request::Status(StatusArgs {
                target: self.target.clone().into(),
                instructions: Input::Missing,
            });
            let response = runtime::client(self.root, &Uuid::new_v4().to_string(), &request)?;
            let reply = api::DaemonReply::new(&response)?;
            if reply.has_error() {
                return wait_error(response, &request);
            }
            let agent = reply.agent()?;
            if self.until == WaitUntil::TurnFinished
                && let Some(result) = self.poll_events(agent.raw())?
            {
                return Ok(result);
            }
            let outcome = agent.outcome();
            if outcome == TaskOutcome::Cancelled {
                return Ok(CommandOutcome::failed(response));
            }
            if outcome == TaskOutcome::Completed || agent.expired() {
                return Ok(CommandOutcome::ok(response));
            }
            if agent.state() == AgentState::Failed {
                return Ok(CommandOutcome::failed(response));
            }
            if agent.held() {
                return Ok(CommandOutcome::failed(api::value(
                    api::WaitObservationResponse {
                        wait: "held",
                        agent: agent.raw(),
                    },
                )));
            }
            if Instant::now() >= self.deadline {
                return Ok(CommandOutcome::failed(api::value(
                    api::WaitObservationResponse {
                        wait: "timeout",
                        agent: agent.raw(),
                    },
                )));
            }
            std::thread::sleep(WAIT_POLL_INTERVAL);
        }
    }
    pub(super) fn poll_events(
        &mut self,
        agent: &serde_json::Value,
    ) -> Result<Option<CommandOutcome>> {
        use uuid::Uuid;

        let request = Request::Read(ReadArgs {
            target: self.target.clone().into(),
            after: self.after.into(),
            limit: WAIT_EVENT_PAGE_ROWS.into(),
        });
        let events = runtime::client(self.root, &Uuid::new_v4().to_string(), &request)?;
        let reply = api::DaemonReply::new(&events)?;
        if reply.has_error() {
            return wait_error(events, &request).map(Some);
        }
        let page = reply.events()?;
        if page.history_expired() {
            return Ok(Some(CommandOutcome::failed(api::value(
                api::HistoryExpiredResponse {
                    error: Error::history_expired("Requested event history was compacted."),
                    pruned_through: page.pruned_through(),
                },
            ))));
        }
        if let Some(event) = page.first_turn_finished() {
            return Ok(Some(CommandOutcome::ok(api::value(
                api::WaitEventResponse { agent, event },
            ))));
        }
        self.after = page.cursor();
        Ok(None)
    }
}

#[derive(clap::Args)]
pub(super) struct Wait {
    #[arg(value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES),
         required_unless_present_any = ["message",
         "join"],
         conflicts_with_all = ["message",
         "join"])]
    /// Agent ID to operate on.
    target: Option<String>,
    /// Resume a wait for an external message; never resends it.
    #[arg(long, value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES),
         conflicts_with_all = ["until",
         "after"])]
    message: Option<String>,
    /// Observe a retained join; timeout never cancels its messages.
    #[arg(long, value_parser = hive_input::clap::parsers::text(
        crate::limits::CONTROL_INPUT_BYTES - "join:".len()
    ),
         conflicts_with_all = ["message",
         "until",
         "after"])]
    join: Option<String>,
    #[arg(
            long,
            value_parser = clap::value_parser!(u64).range(MIN_TIMEOUT_SECONDS..=MAX_WAIT_SECONDS)
        )]
    /// Wait deadline in seconds: 1-3600 for agents, 1-600 for messages/joins (defaults 60/45).
    timeout: Option<u64>,
    #[arg(long, default_value = "completed", value_enum)]
    /// Agent condition: completed or turn-finished (default completed).
    until: WaitUntil,
    /// Exclusive event cursor (default 0); requires --until turn-finished.
    #[arg(long)]
    after: Option<u64>,
}

impl Wait {
    pub(super) fn ignored_cursor(&self) -> bool {
        self.after.is_some() && self.until != WaitUntil::TurnFinished
    }
    /// Conditional bounds must be checked after clap has selected the wait mode.
    pub(super) fn invalid_reply_timeout(&self) -> Option<u64> {
        self.timeout.filter(|timeout| {
            (self.join.is_some() || self.message.is_some()) && *timeout > crate::calls::MAX_TIMEOUT
        })
    }
    #[expect(
        clippy::expect_used,
        reason = "Clap requires either an agent target or a message ID"
    )]
    pub(super) fn run(&self, root: &std::path::Path) -> Result<CommandOutcome> {
        if let Some(join) = &self.join {
            return super::join_wait::run(
                root,
                join,
                self.timeout.unwrap_or(crate::calls::DEFAULT_TIMEOUT),
            );
        }
        if let Some(message) = &self.message {
            let result = crate::calls::executor()?.block_on(crate::calls::wait(
                root,
                message,
                self.timeout.unwrap_or(crate::calls::DEFAULT_TIMEOUT),
                crate::calls::cli_progress(),
            ));
            return Ok(result.into());
        }
        Waiter {
            root,
            target: self
                .target
                .clone()
                .expect("Clap requires an agent or message"),
            until: self.until,
            after: self.after.unwrap_or(0),
            deadline: Instant::now()
                + std::time::Duration::from_secs(
                    self.timeout.unwrap_or(super::DEFAULT_WAIT_SECONDS),
                ),
        }
        .run()
    }
}

fn wait_error(value: serde_json::Value, request: &Request) -> Result<CommandOutcome> {
    let reply = api::DaemonReply::new(&value)?;
    let input = super::remote::input_reply(&reply, request)?
        .map(super::remote::InputReply::for_wait)
        .transpose()?;
    Ok(CommandOutcome {
        value: input
            .as_ref()
            .map_or(value, super::remote::InputReply::value),
        success: false,
        input,
    })
}
