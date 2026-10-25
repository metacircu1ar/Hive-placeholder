//! Drive worker admission, active turns, interruption, and deadlines.
use super::steering::acknowledgement;
use super::{
    ActiveTurn, AdmissionEvent, AdmissionReadiness, AgentState, Control, Duration, Error,
    INTERRUPT_CONFIRMATION_TIMEOUT, IncomingFrame, Input, Instant, Message, MessageState,
    NO_DEADLINE_WAIT, Ordering, Result, SessionEvent, SessionLoop, TURN_TIMEOUT, TurnAdmission,
    handle, models, steering,
};
use crate::config::Provider;
use crate::runtime::api::events;
use crate::runtime::domain::AuthCutoff;
use crate::runtime::provider::claude::MAX_HELD;

struct TurnRead {
    state: super::SessionState,
    guidance: Option<Message>,
    redirect: bool,
}

// Codex acknowledges guidance individually; Claude admits its bounded hook batch.
const CODEX_STEERING_WINDOW: usize = 1;
// Completion clears coordinating activity until the next admission refreshes placement.
const IDLE_ACTIVITY_CLASS: usize = 2;

impl SessionLoop<'_> {
    pub(super) async fn run(&mut self) -> Result<bool> {
        // Each turn orders fenced storage decisions, provider I/O and admission waits.
        loop {
            let TurnRead {
                state,
                guidance,
                redirect,
            } = self.read_turn().await?;
            self.state = state;
            let stopping = self.shutdown.load(Ordering::Relaxed)
                || self.state.control == Some(Control::Stop)
                || self.state.terminal;
            self.interrupt_if_requested(stopping, redirect).await?;
            if self.should_stop(stopping)? {
                break;
            }
            if !stopping
                && !self.interrupt_sent
                && let Some(message) = guidance
            {
                self.start_steering(message).await?;
                continue;
            }
            if self.active.is_none()
                && self.pending_steering.is_empty()
                && !self.state.paused
                && !stopping
                && self.admit_work().await?
            {
                continue;
            }
            self.wait_for_event().await?;
        }
        Ok(self.active.is_some() || !self.pending_steering.is_empty())
    }

    #[expect(
        clippy::expect_used,
        reason = "ExecutionHandle::agent validates the live execution ID before redirect \
            claiming."
    )]
    async fn read_turn(&mut self) -> Result<TurnRead> {
        let poll_steering = self.active.is_some()
            && self.pending_steering.len()
                < if self.rpc.codex() {
                    CODEX_STEERING_WINDOW
                } else {
                    MAX_HELD
                };
        let redirect_target = self
            .active
            .as_ref()
            .filter(|_| !self.interrupt_sent)
            .map(|active| (active.message.id.clone(), active.turn_id.clone()));
        let read = self
            .execution
            .agent(move |core, agent| {
                if agent.completion_pending.is_some() && agent.turn_id.is_none() {
                    core.finish_task(agent)?;
                }
                let guidance = if poll_steering {
                    steering::queued(core, agent)?
                } else {
                    None
                };
                let redirect = if agent.redirect_pending
                    && let Some((message, turn)) = redirect_target
                {
                    core.store().claim_redirect(
                        &agent.id,
                        agent.execution_id.as_deref().expect("active execution"),
                        &message,
                        &turn,
                    )?
                } else {
                    false
                };
                Ok(TurnRead {
                    state: super::SessionState::from(&*agent),
                    guidance,
                    redirect,
                })
            })
            .await?;
        Ok(read)
    }

    pub(super) async fn interrupt_if_requested(
        &mut self,
        stopping: bool,
        redirect: bool,
    ) -> Result<()> {
        if (stopping || self.state.control == Some(Control::Interrupt) || redirect)
            && !self.interrupt_sent
        {
            if let Some(turn) = self.active.as_ref().map(|active| active.turn_id.clone()) {
                // Retire hosted callback ownership before native interruption can
                // emit its terminal result in the same read as the interrupt ACK.
                #[cfg(feature = "comb")]
                self.rpc.forget_comb_turn(&turn).await?;
                #[cfg(feature = "comb")]
                self.abandon_comb().await?;
                self.rpc.interrupt(self.thread_id, &turn).await?;
                self.interrupt_sent = true;
                self.stop_deadline = Some(Instant::now() + INTERRUPT_CONFIRMATION_TIMEOUT);
            } else if !stopping {
                self.execution
                    .agent(|core, agent| {
                        if agent.control == Some(Control::Interrupt) {
                            agent.control = None;
                            core.store().save(agent)?;
                        }
                        Ok(())
                    })
                    .await?;
            }
        }
        Ok(())
    }

    pub(super) fn should_stop(&self, stopping: bool) -> Result<bool> {
        if self.active.is_none()
            && self.pending_steering.is_empty()
            && (stopping
                || self.rpc.draining()
                || self.relaunch
                || self.state.waiting_reason.is_some()
                || self.state.branch_hold > 0
                || (self.idle_seconds > 0
                    && self.idle_at.elapsed() >= Duration::from_secs(self.idle_seconds)))
        {
            return Ok(true);
        }
        if self.stop_deadline.is_some_and(|d| Instant::now() >= d) {
            if stopping {
                return Ok(true);
            }
            return Err(Error::interrupt_timeout(
                "Provider did not confirm interruption.",
            ));
        }
        Ok(false)
    }

    /// True means resume the outer loop, including its lifecycle recheck.
    pub(super) async fn admit_work(&mut self) -> Result<bool> {
        // Only hold a turn permit when an inbox actually contains work.
        let AdmissionReadiness { waiting, placement } = self
            .execution
            .agent(|core, agent| {
                Ok(AdmissionReadiness {
                    waiting: core.store().next_message(&agent.id)?.is_some(),
                    placement: core.store().placement(&agent.id)?,
                })
            })
            .await?;
        self.rpc.activity(placement, false, waiting, false);
        if !waiting {
            self.admission.take();
        }
        if waiting && self.permit.is_none() && self.await_admission().await? {
            return Ok(true);
        }
        if self.permit.is_some() {
            self.start_message(placement).await?;
        }
        Ok(self.relaunch)
    }

    pub(super) async fn await_admission(&mut self) -> Result<bool> {
        let admission = self
            .admission
            .get_or_insert_with(|| Box::pin(self.turns.clone().acquire_owned()));
        let event = tokio::select! {
            permit = admission => AdmissionEvent::Permit(permit),
            frame = self.rpc.recv() => AdmissionEvent::Frame(frame),
            () = self.notify.notified() => AdmissionEvent::Wake,
        };
        match event {
            AdmissionEvent::Permit(permit) => {
                *self.permit = Some(permit.map_err(|cause| {
                    Error::daemon_stopping("Turn scheduler stopped.").with_source(cause)
                })?);
                self.admission.take();
                Ok(false)
            }
            AdmissionEvent::Frame(frame) => {
                // Admission frames intentionally skip finished-turn bookkeeping.
                self.handle_frame(frame?).await?;
                Ok(true)
            }
            AdmissionEvent::Wake => Ok(true),
        }
    }

    pub(super) async fn record_branch_input(&mut self) -> Result<()> {
        if !self.rpc.codex() && self.state.branch_unbound {
            self.execution
                .agent(|core, agent| {
                    if agent.branch_unbound() {
                        let thread = agent.thread_id.as_deref().ok_or_else(|| {
                            Error::session_mismatch("Missing Claude branch reservation.")
                        })?;
                        core.store().record_branch_native(agent, thread)?;
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }
    pub(super) async fn start_message(&mut self, placement: usize) -> Result<()> {
        let loaded = self.selection.clone();
        let provider_version = self.rpc.provider_version().map(str::to_owned);
        let admission = self
            .execution
            .agent(move |core, agent| {
                models::claim(core, agent, &loaded, provider_version.as_deref())
            })
            .await?;
        if let TurnAdmission::Submit {
            message,
            selection,
            previously_attempted,
        } = admission
        {
            self.submit_message(placement, *message, selection, previously_attempted)
                .await?;
        } else {
            self.relaunch = matches!(admission, TurnAdmission::Relaunch);
            self.permit.take();
        }
        Ok(())
    }

    async fn submit_message(
        &mut self,
        placement: usize,
        message: Message,
        selection: super::OptionsSelection,
        previously_attempted: bool,
    ) -> Result<()> {
        let prepared = self.prepare_attachments(message, &selection).await;
        self.rpc.activity(placement, true, false, true);
        let (message, result) = match prepared {
            Ok(prepared) => (
                Some(prepared.message),
                self.rpc.call(prepared.request).await,
            ),
            Err(error) => (None, Err(error.before_submission())),
        };
        if !previously_attempted
            && result.as_ref().is_err_and(|e| {
                matches!(e, Error::ProviderDraining { .. }) || e.input_was_not_submitted()
            })
        {
            // This exact error is emitted before begin_request/codec.send.
            // Only this first, proven-unsent attempt may release its reservation.
            self.execution
                .agent(|core, agent| {
                    agent.native_input_attempted = false;
                    core.store().save(agent)
                })
                .await?;
        }
        let result = result?;
        let message = message
            .ok_or_else(|| Error::state_inconsistent("Submitted attachment input is missing."))?;
        let turn = Input::<String>::from_value(result.raw_turn_id().clone())
            .protocol_text()?
            .to_owned();
        let saved = Message {
            has_attachments: message.has_attachments,
            attachments: message.attachments.clone(),
            joined: message.joined,
            id: message.id.clone(),
            sender: message.sender.clone(),
            recipient: message.recipient.clone(),
            body: String::new(),
            state: message.state,
        };
        let saved_turn = turn.clone();
        self.record_branch_input().await?;
        let accepted = selection.clone();
        if self.record_started(saved, saved_turn, selection).await? {
            self.rpc.commit_branch(&turn).await?;
        }
        self.selection = accepted;
        self.applied_turn_selection = self.selection.clone();
        self.active = Some(ActiveTurn {
            message,
            turn_id: turn,
            started: Instant::now(),
        });
        self.interrupt_sent = false;
        self.stop_deadline = None;
        Ok(())
    }

    async fn record_started(
        &mut self,
        saved: Message,
        saved_turn: String,
        selection: super::OptionsSelection,
    ) -> Result<bool> {
        self.execution
            .agent(move |core, agent| {
                let branching =
                    agent.branch_unbound() && agent.profile.provider == Provider::Claude;
                if branching {
                    let thread = agent.thread_id.clone().ok_or_else(|| {
                        Error::session_mismatch("Missing Claude branch reservation.")
                    })?;
                    core.store().bind_branch(agent, &thread)?;
                }
                if agent
                    .auth_cutoff
                    .as_ref()
                    .is_some_and(|cutoff| cutoff.turn() == saved_turn)
                {
                    agent.auth_cutoff = Some(AuthCutoff::Bound {
                        turn: saved_turn.clone(),
                        message: saved.id.clone(),
                    });
                } else {
                    agent.clear_cutoff();
                }
                agent.turn_id = Some(saved_turn.clone());
                agent.state = AgentState::Running;
                agent.reserved_turn = None;
                agent.last_accepted_selection = Some(selection.clone());
                agent.applied_options_revision = Some(selection.revision);
                core.store()
                    .message_state(&saved, MessageState::Submitted)?;
                core.store().save(agent)?;
                core.store().turn_started(
                    agent,
                    events::TurnStarted {
                        attachments: &saved.attachments,
                        turn_id: &saved_turn,
                        message_id: &saved.id,
                        sender: &saved.sender,
                        selection: events::TurnSelection {
                            model: &selection.model,
                            effort: &selection.effort,
                            options_revision: selection.revision,
                            selection_verification: if agent.profile.provider == Provider::Claude {
                                "session_verified"
                            } else {
                                "requested_unverified"
                            },
                        },
                    },
                )?;
                Ok(branching)
            })
            .await
    }

    pub(super) fn deadline(&self) -> Instant {
        let active_deadline = self
            .active
            .as_ref()
            .map(|ActiveTurn { started: start, .. }| *start + TURN_TIMEOUT);
        let deadline = active_deadline
            .into_iter()
            .chain(self.stop_deadline)
            .chain(
                self.pending_steering
                    .iter()
                    .filter_map(|pending| pending.deadline),
            )
            .min();
        // An idle session uses its idle expiry even when a stop deadline remains.
        let deadline =
            if self.active.is_none() && self.pending_steering.is_empty() && self.idle_seconds > 0 {
                Some(self.idle_at + Duration::from_secs(self.idle_seconds))
            } else {
                deadline
            };
        deadline.unwrap_or_else(|| Instant::now() + NO_DEADLINE_WAIT)
    }

    pub(super) async fn wait_for_event(&mut self) -> Result<()> {
        let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(self.deadline()));
        let event = {
            #[cfg(feature = "comb")]
            let knowledge = async {
                let (index, result) = super::comb::response(&mut self.pending_comb).await;
                SessionEvent::Comb(index, result)
            };
            #[cfg(not(feature = "comb"))]
            let knowledge = std::future::pending::<SessionEvent>();
            tokio::select! {
                frame = self.rpc.recv() => SessionEvent::Frame(frame),
                event = knowledge => event,
                result = acknowledgement(&mut self.pending_steering) => {
                    SessionEvent::Steering(result)
                },

                () = self.notify.notified() => SessionEvent::Wake,
                () = sleep => SessionEvent::Deadline,
            }
        };
        match event {
            SessionEvent::Frame(frame) => self.handle_running_frame(frame).await,
            SessionEvent::Steering(steering::Acknowledgement { index, result }) => {
                self.finish_steering(index, result).await
            }
            #[cfg(feature = "comb")]
            SessionEvent::Comb(index, result) => self.finish_comb(index, result).await,
            SessionEvent::Wake => Ok(()),
            SessionEvent::Deadline => self.check_turn_deadline(),
        }
    }

    pub(super) async fn handle_frame(&mut self, frame: IncomingFrame) -> Result<()> {
        #[cfg(feature = "comb")]
        if self.start_comb(&frame).await? {
            return Ok(());
        }
        handle(
            self.execution,
            self.rpc,
            self.thread_id,
            &mut self.active,
            frame,
        )
        .await
    }

    pub(super) async fn handle_running_frame(
        &mut self,
        frame: Result<IncomingFrame>,
    ) -> Result<()> {
        let frame = frame?;
        let completion = MessageState::from_provider_completion(frame.params().turn_status());
        let was_active = self.active.is_some();
        self.handle_frame(frame).await?;
        if was_active && self.active.is_none() {
            #[cfg(feature = "comb")]
            self.abandon_comb().await?;
            for pending in &mut self.pending_steering {
                pending.completion = Some(completion);
            }
            self.rpc.activity(IDLE_ACTIVITY_CLASS, false, false, true);
            self.permit.take();
            self.interrupt_sent = false;
            self.stop_deadline = None;
            self.idle_at = Instant::now();
        }
        Ok(())
    }

    pub(super) fn check_turn_deadline(&self) -> Result<()> {
        if self.pending_steering.iter().any(|p| {
            p.deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        }) {
            return Err(Error::provider_timeout(
                "Steering delivery did not settle within 30 seconds.",
            ));
        }
        if self
            .active
            .as_ref()
            .is_some_and(|ActiveTurn { started: start, .. }| start.elapsed() >= TURN_TIMEOUT)
        {
            return Err(Error::turn_timeout("Agent turn exceeded 30 minutes."));
        }
        Ok(())
    }
}
