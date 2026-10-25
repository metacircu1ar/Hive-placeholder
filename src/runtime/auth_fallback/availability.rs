//! Subscription availability records and provider evidence transitions.
use super::AuthReason;
use super::signals::Signal;
use crate::runtime::api::{self, events};
use crate::{
    error::Result,
    runtime::{
        Core,
        domain::{Agent, ProviderEventKind},
    },
};

use super::ProbeError;
use crate::runtime::domain::{decode, domain_enum};
use serde::{Deserialize, Serialize};

domain_enum!(AvailabilityState {
    Available => "available", Limited => "limited", Unavailable => "unavailable",
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Probe {
    pub id: String,
    pub agent: String,
    pub message: String,
    pub deadline: u64,
    pub error: Option<ProbeError>,
}

// One field declaration serves the grouped state and the exact flat SQL/API record.
macro_rules! availability_record {
    ($($(#[$attribute:meta])* $field:ident: $kind:ty),+ $(,)?) => {
        #[derive(Clone, Debug)]
        pub struct Availability {
            $(pub $field: $kind,)+
            pub probe: Option<Probe>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct AvailabilityRecord {
            $($(#[$attribute])* $field: $kind,)+
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            probe_id: Option<String>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            probe_agent: Option<String>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            probe_message: Option<String>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            probe_deadline: Option<u64>,
            #[serde(deserialize_with = "serde::Deserialize::deserialize")]
            probe_error: Option<ProbeError>,
        }
        impl<'de> Deserialize<'de> for Availability {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let record: AvailabilityRecord = decode::object(deserializer)?;
                let probe = match (
                    record.probe_id,
                    record.probe_agent,
                    record.probe_message,
                    record.probe_deadline,
                    record.probe_error,
                ) {
                    (None, None, None, None, None) => None,
                    (Some(id), Some(agent), Some(message), Some(deadline), error) => {
                        Some(Probe { id, agent, message, deadline, error })
                    }
                    _ => return Err(serde::de::Error::custom("Incomplete quota probe identity.")),
                };
                Ok(Self { $($field: record.$field,)+ probe })
            }
        }
        impl Serialize for Availability {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(None)?;
                $(map.serialize_entry(stringify!($field), &self.$field)?;)+
                map.serialize_entry("probe_id", &self.probe_id())?;
                map.serialize_entry("probe_agent", &self.probe_agent())?;
                map.serialize_entry("probe_message", &self.probe_message())?;
                map.serialize_entry("probe_deadline", &self.probe_deadline())?;
                map.serialize_entry("probe_error", &self.probe_error())?;
                map.end()
            }
        }
    };
}
availability_record! {
    profile: String,
    generation: uuid::Uuid,
    revision: u64,
    #[serde(deserialize_with = "decode::optional_object")]
    key: Option<crate::state::KeyAuthority>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    state: AvailabilityState,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    until: Option<u64>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    retry_at: Option<u64>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    window: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    reason: Option<AuthReason>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    confirmed_at: Option<u64>,
    updated_at: u64,
}
impl Availability {
    pub fn available(profile: &str, generation: uuid::Uuid) -> Self {
        Self {
            profile: profile.into(),
            generation,
            revision: 0,
            key: None,
            state: AvailabilityState::Available,
            until: None,
            retry_at: None,
            probe: None,
            window: None,
            reason: None,
            confirmed_at: None,
            updated_at: crate::state::now(),
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        self.until.or(self.retry_at)
    }
    pub fn usable(&self, now: u64, agent: Option<&str>) -> bool {
        self.probe
            .as_ref()
            .is_none_or(|probe| probe.error.is_none() && Some(probe.agent.as_str()) == agent)
            && (self.state == AvailabilityState::Available
                || (self.state == AvailabilityState::Limited
                    && self.until.is_some_and(|at| at <= now)))
    }
    pub fn mark_available(&mut self) {
        self.state = AvailabilityState::Available;
        self.until = None;
        self.retry_at = None;
        self.reason = None;
        self.window = None;
    }
    pub fn clear_probe(&mut self) {
        self.probe = None;
    }
    pub fn probe_id(&self) -> Option<&str> {
        self.probe.as_ref().map(|probe| probe.id.as_str())
    }
    pub fn probe_agent(&self) -> Option<&str> {
        self.probe.as_ref().map(|probe| probe.agent.as_str())
    }
    pub fn probe_message(&self) -> Option<&str> {
        self.probe.as_ref().map(|probe| probe.message.as_str())
    }
    pub fn probe_deadline(&self) -> Option<u64> {
        self.probe.as_ref().map(|probe| probe.deadline)
    }
    pub fn probe_error(&self) -> Option<ProbeError> {
        self.probe.as_ref().and_then(|probe| probe.error)
    }
}

impl Core {
    pub(in crate::runtime) fn auth_signal(
        &mut self,
        profile: &str,
        generation: uuid::Uuid,
        provider: &str,
        signal: Signal,
    ) -> Result<()> {
        let mut value = self.store().availability(profile, generation)?;
        if value.key != signal.key {
            return Ok(());
        }
        if value.state == AvailabilityState::Unavailable
            && value.reason == Some(AuthReason::SpendCap)
        {
            return Ok(());
        }
        let now = crate::state::now();
        if sparse_signal(&value, &signal, now) {
            return Ok(());
        }
        let held = value.state != AvailabilityState::Available || value.retry_at.is_some();
        let reason = combined_reason(&value, &signal);
        let state = if value.state == AvailabilityState::Unavailable {
            AvailabilityState::Unavailable
        } else {
            signal.state
        };
        let until = signal_until(&value, &signal, now, held, reason);
        let retry_at = if value.key.is_some() {
            None
        } else {
            signal_retry(&value, &signal, now, state, reason, until)
        };
        if value.state == state
            && value.until == until
            && value.retry_at == retry_at
            && value.reason == Some(reason)
        {
            return Ok(());
        }

        value.state = state;
        value.until = until;
        value.retry_at = retry_at;
        value.reason = Some(reason);
        value.window = signal.window;
        value.updated_at = now;
        value.confirmed_at = None;
        // A new limit does not prove that a probe's native turn stopped.
        self.store().save_revised(&mut value)?;
        self.store().provider_event(
            provider,
            ProviderEventKind::LoginLimited,
            api::value(events::LoginLimited {
                profile,
                generation,
                state: value.state,
                until: value.until,
                retry_at: value.retry_at,
                window: value.window.as_deref(),
                reason: value.reason,
                rejected_resets: &signal.rejected_resets,
                method: if value.key.is_some() {
                    "api_key"
                } else {
                    "subscription"
                },
            }),
        )
    }

    pub(in crate::runtime) fn auth_live_read(
        &mut self,
        profile: &str,
        generation: uuid::Uuid,
        provider: &str,
        observation: (u64, Option<crate::state::KeyAuthority>),
        read: super::signals::CodexRead,
    ) -> Result<()> {
        let mut value = self.store().availability(profile, generation)?;
        if value.revision != observation.0
            || value.key != observation.1
            || value.probe_error().is_some()
        {
            return Ok(());
        }
        if read.confirms_usage()
            && value.reason == Some(AuthReason::UsageLimit)
            && value.state != AvailabilityState::Unavailable
        {
            value.mark_available();
            value.confirmed_at = Some(crate::state::now());
            if let (Some(owner), Some(message)) = (&value.probe_agent(), &value.probe_message())
                && self.store().probe_input(owner, message)?
                    == Some(crate::runtime::domain::MessageState::Accepted)
            {
                value.clear_probe();
            }

            value.updated_at = crate::state::now();
            return self.store().save_revised(&mut value);
        }
        let mut limits = super::signals::CodexLimits::default();
        limits.merge(read.rate_limits);
        let signal = limits.signal().or_else(|| {
            (read.ordinary_usage_allowed == Some(false))
                .then(|| limits.exhaustion(crate::state::now()))
        });
        if let Some(mut signal) = signal {
            signal.key = observation.1;
            self.auth_signal(profile, generation, provider, signal)?;
        }
        Ok(())
    }

    pub(in crate::runtime) fn confirm_login_turn(&self, agent: &Agent) -> Result<()> {
        let mut value = self
            .store()
            .availability(&agent.auth, agent.auth_generation)?;
        if value.probe_error().is_some()
            || agent.turn_auth_revision != Some(value.revision)
            || agent.execution_key != value.key
        {
            return Ok(());
        }
        if value.state == AvailabilityState::Available && value.confirmed_at.is_some() {
            return Ok(());
        }

        value.mark_available();
        value.confirmed_at = Some(crate::state::now());
        value.updated_at = crate::state::now();
        self.store().save_revised(&mut value)
    }
}

fn combined_reason(value: &super::Availability, signal: &Signal) -> AuthReason {
    let held = value.state != AvailabilityState::Available || value.retry_at.is_some();
    let mixed = held
        && (value.reason == Some(AuthReason::UsageAndAuthentication)
            || matches!(
                (value.reason, signal.reason),
                (Some(AuthReason::UsageLimit), AuthReason::Authentication)
                    | (Some(AuthReason::Authentication), AuthReason::UsageLimit)
            ));
    if mixed && signal.reason != AuthReason::SpendCap {
        AuthReason::UsageAndAuthentication
    } else {
        signal.reason
    }
}

fn signal_until(
    value: &Availability,
    signal: &Signal,
    now: u64,
    held: bool,
    reason: AuthReason,
) -> Option<u64> {
    let mut until = signal.until;
    if held && signal.state == AvailabilityState::Limited {
        until = if value.retry_at.is_some()
            || (value.state == AvailabilityState::Limited && value.until.is_none())
        {
            None // A newly known window cannot erase an unconfirmed unknown window.
        } else {
            match (until, value.until.filter(|at| *at > now)) {
                (Some(next), Some(current)) => Some(next.max(current)),
                (None, Some(current))
                    if signal.window.is_none() && signal.reason == AuthReason::UsageLimit =>
                {
                    Some(current)
                }
                (next, _) => next,
            }
        };
    }
    if matches!(
        reason,
        AuthReason::Authentication | AuthReason::UsageAndAuthentication
    ) {
        until = None;
    }
    until
}
#[expect(
    clippy::expect_used,
    reason = "The maximum always includes Some(base), independently of optional reset times."
)]
fn signal_retry(
    value: &Availability,
    signal: &Signal,
    now: u64,
    state: AvailabilityState,
    reason: AuthReason,
    until: Option<u64>,
) -> Option<u64> {
    if (state == AvailabilityState::Limited && until.is_none())
        || (state == AvailabilityState::Unavailable && reason == AuthReason::UsageAndAuthentication)
    {
        let base = if value.state != AvailabilityState::Available && value.reason == Some(reason) {
            value
                .retry_at
                .unwrap_or(now + super::probes::retry_seconds(Some(reason)))
        } else {
            now + super::probes::retry_seconds(Some(reason))
        };
        Some(
            [
                Some(base),
                value.retry_at,
                value.until,
                signal.until,
                signal.reset_floor,
            ]
            .into_iter()
            .flatten()
            .max()
            .expect("retry base"),
        )
    } else {
        None
    }
}

// Sparse telemetry cannot revoke a granted retry or its outstanding cause.
fn sparse_signal(value: &Availability, signal: &Signal, now: u64) -> bool {
    let permitted = value.state == AvailabilityState::Available && value.retry_at.is_some();
    permitted
        && signal.state == AvailabilityState::Limited
        && !signal.turn_error
        && !signal
            .until
            .into_iter()
            .chain(signal.reset_floor)
            .any(|at| at > now)
}
