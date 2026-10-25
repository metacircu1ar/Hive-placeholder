//! Track outgoing provider requests and validate matching responses.
use super::{
    ByteQueue, ClientMethod, Duration, Error, IncomingFrame, Instant, PENDING_LIMIT,
    PROTOCOL_ID_BYTES, REQUEST_TIMEOUT, Request, Response, Result, Rpc, Turn, outgoing, protocol,
};
use crate::runtime::provider::error::{ACK_TIMEOUT, CLEANING_UP};
use std::mem;

impl Rpc {
    /// Begin without waiting for an ACK. The shard routes other conversations
    /// while any request is outstanding. The request ID is connection-local.
    // Consume request parameters at this lifecycle transition, retaining
    // the original drop boundary.
    pub(crate) fn begin_request(&mut self, request: outgoing::OutgoingRequest) -> Result<u64> {
        let method = request.method();
        if (self.protocol_uncertain || self.has_quarantined) && method.submits_input() {
            return Err(if method == ClientMethod::ThreadFork {
                protocol().before_submission()
            } else {
                protocol()
            });
        }
        let thread = request.thread().map(str::to_owned);
        let expected_turn = request.expected_turn().map(str::to_owned);
        let fork_cutoff = request.fork_cutoff().map(str::to_owned);
        match method {
            method if method.opens_thread() => {
                self.opening += 1;
            }
            ClientMethod::TurnStart => {
                let id = thread.as_ref().ok_or_else(protocol)?;
                let state = self.threads.get_mut(id).ok_or_else(protocol)?;
                if !matches!(state.turn, Some(Turn::Idle)) || state.drain.is_some() {
                    return Err(protocol());
                }
                state.completed = None;
                state.turn = Some(Turn::Submitting);
            }
            _ => {}
        }
        self.next_id += 1;
        let id = self.next_id;
        self.requests.insert(
            id,
            Request {
                method,
                thread,
                expected_turn,
                fork_cutoff,
            },
        );
        self.send(outgoing::request(id, request))?;
        Ok(id)
    }
    pub(crate) fn finish_response(&mut self, value: &IncomingFrame) -> Result<Response> {
        let id = value.id().ok_or_else(protocol)?;
        let request = self.requests.remove(&id).ok_or_else(protocol)?;
        if value.has_result() && value.has_error() {
            if let Some(thread) = &request.thread
                && request.method != ClientMethod::ThreadFork
            {
                self.quarantine(thread);
            } else {
                self.protocol_uncertain = true;
            }
            return Err(protocol());
        }
        let result = if let Some(result) = value.result() {
            Ok(result)
        } else if value.valid_error() {
            Err(if request.method == ClientMethod::TurnSteer {
                value.steering_error(request.expected_turn.as_deref())
            } else if request.method == ClientMethod::ThreadFork {
                value.fork_error(request.fork_cutoff.as_deref())
            } else {
                Error::provider_request_failed(
                    "Provider rejected the request; check selected model, permissions and \
                    account access.",
                )
            })
        } else {
            if let Some(thread) = &request.thread
                && request.method != ClientMethod::ThreadFork
            {
                self.quarantine(thread);
            } else {
                self.protocol_uncertain = true;
            }
            return Err(protocol());
        };
        match request.method {
            ClientMethod::TurnSteer => {
                if let Ok(value) = &result
                    && (request.expected_turn.is_none()
                        || value.steered_turn_id() != request.expected_turn.as_deref())
                {
                    if let Some(thread) = &request.thread {
                        self.quarantine(thread);
                    }
                    return Err(protocol());
                }
            }
            method if method.opens_thread() => self.finish_open_response(&request, &result)?,
            ClientMethod::TurnStart => {
                self.finish_turn_response(request, &result)?;
            }
            _ => {}
        }
        result
    }
    fn finish_turn_response(&mut self, request: Request, result: &Result<Response>) -> Result<()> {
        let thread = request.thread.ok_or_else(protocol)?;
        let state = if let Ok(v) = result {
            let Some(turn) = v
                .turn_id()
                .filter(|s| !s.is_empty() && s.len() <= PROTOCOL_ID_BYTES)
            else {
                self.quarantine(&thread);
                return Err(protocol());
            };
            Turn::Active(turn.into())
        } else {
            Turn::Idle
        };
        if let Some(observed) = self
            .threads
            .get_mut(&thread)
            .and_then(|state| state.early_item.take())
            && !matches!(&state,Turn::Active(id) if id==&observed)
        {
            self.quarantine(&thread);
        }
        let current = self.threads.entry(thread).or_default();
        current.turn = Some(state);
        let early = mem::take(&mut current.early);
        for value in early {
            self.observe(&value);
        }
        Ok(())
    }
    fn finish_open_response(&mut self, request: &Request, result: &Result<Response>) -> Result<()> {
        if let Ok(v) = &result {
            let thread = v
                .thread_id()
                .filter(|s| !s.is_empty() && s.len() <= PROTOCOL_ID_BYTES)
                .ok_or_else(protocol)?;
            if (request.method == ClientMethod::ThreadResume
                && request.thread.as_deref() != Some(thread))
                || (request.method == ClientMethod::ThreadFork
                    && (request.thread.as_deref() == Some(thread) || self.turn(thread).is_some()))
                || self
                    .turn(thread)
                    .is_some_and(|turn| !matches!(turn, Turn::Idle))
            {
                self.protocol_uncertain = true;
                return Err(protocol());
            }
            self.threads.entry(thread.into()).or_default().turn = Some(Turn::Idle);
        }
        self.opening -= 1;

        Ok(())
    }
    pub(crate) fn call(&mut self, request: outgoing::OutgoingRequest) -> Result<Response> {
        self.call_with_timeout(request, REQUEST_TIMEOUT)
    }
    pub(super) fn call_with_timeout(
        &mut self,
        request: outgoing::OutgoingRequest,
        timeout: Duration,
    ) -> Result<Response> {
        let id = self.begin_request(request)?;
        let deadline = Instant::now() + timeout;
        self.response_until(id, deadline)
    }
    pub(super) fn call_until(
        &mut self,
        request: outgoing::OutgoingRequest,
        deadline: Instant,
    ) -> Result<Response> {
        let id = self.begin_request(request)?;
        self.response_until(id, deadline)
    }
    fn response_until(&mut self, id: u64, deadline: Instant) -> Result<Response> {
        let mut deferred = ByteQueue::<IncomingFrame>::default();
        let result = self.await_response(id, deadline, &mut deferred);
        deferred.append(&mut self.pending);
        self.pending = deferred;
        result
    }
    fn await_response(
        &mut self,
        id: u64,
        deadline: Instant,
        deferred: &mut ByteQueue<IncomingFrame>,
    ) -> Result<Response> {
        while Instant::now() < deadline {
            let value = match self.pending.pop_front() {
                Some(v) => Some(v),
                None => self.next()?,
            };
            if let Some(value) = value {
                if value.matches_response(id) {
                    return self.finish_response(&value);
                }
                if value.is_response() {
                    self.reconcile_response(&value)?;
                    continue;
                }
                if value.is_request() {
                    self.send(outgoing::unsupported(value.raw_id(), CLEANING_UP))?;
                    continue;
                }
                // Authentication notices may echo a key. They are not agent events.
                if self.private_bootstrap {
                    continue;
                }
                let bytes = value.encoded_len();
                if !deferred.fits(
                    self.pending.len() + 1,
                    self.pending.bytes() + bytes,
                    PENDING_LIMIT,
                ) {
                    return Err(Error::output_limit("Too many pending provider events."));
                }
                deferred.push_back(value, bytes);
            } else {
                self.wait(None, deadline.saturating_duration_since(Instant::now()))?;
            }
        }
        // Retain even metadata request identities until their late ACK or
        // process exit. Otherwise a valid late reply looks unsolicited and
        // unnecessarily poisons an otherwise quiescent process.
        Err(Error::provider_timeout(ACK_TIMEOUT))
    }
    pub(super) fn reconcile_response(&mut self, value: &IncomingFrame) -> Result<()> {
        if !value.id().is_some_and(|id| self.requests.contains_key(&id)) {
            self.poison();
            return Err(protocol());
        }
        // Unrelated acknowledgements still settle lifecycle state; native rejection
        // belongs to that original request, not the current synchronous wait.
        if let Err(error) = self.finish_response(value) {
            tracing::debug!(name: "provider.late_response_failed",
                 pid = self.pipes.child.id(),
                 code = error.code(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "preserve_current_request");
        }
        Ok(())
    }
    pub(crate) fn initialize(&mut self) -> Result<()> {
        let reply = self
            .call(outgoing::initialize(self.live.is_some()))?
            .into_raw();
        if self.claude.is_none() {
            // Metadata is private provider evidence, never a tracing payload.
            // Bound retained startup evidence independently of native frame size.
            if serde_json::to_vec(&reply)
                .map_err(|cause| {
                    Error::provider_protocol_error("Invalid initialization metadata.")
                        .with_source(cause)
                })?
                .len()
                > 16 * 1024
            {
                return Err(Error::output_limit(
                    "Provider initialization metadata exceeds 16 KiB.",
                ));
            }
            self.initialization = Some(reply);
        }
        self.send(outgoing::initialized())
    }
    pub(crate) fn interrupt(&mut self, thread: &str, turn: &str) -> Result<()> {
        match self.call(outgoing::interrupt(thread, turn)) {
            // The turn may have ended while this request was in flight. Both callers
            // still require matching completion within their bounded wait.
            Ok(_) | Err(Error::ProviderRequestFailed { .. }) => Ok(()),
            Err(error) => Err(error),
        }
    }
}
