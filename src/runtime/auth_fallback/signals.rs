//! Only allowlisted, typed provider fields influence availability. Raw frames stay ephemeral.
use super::AuthReason;
use super::AvailabilityState;
use serde::Deserialize;
use serde_json::Value;

// Native reset timestamps more than 32 days ahead are rejected as implausible.
const MAX_RESET_DELAY_SECONDS: u64 = 32 * 86_400;
// A fully consumed quota window is unavailable until its reset boundary.
const EXHAUSTED_PERCENT: f64 = 100.0;

use crate::runtime::provider::protocol::wire_enum;
wire_enum!(CodexErrorInfo {
    UsageLimitExceeded => "usageLimitExceeded",
    Unauthorized => "unauthorized",
    BillingError => "billingError",
});
wire_enum!(LimitReached {
    OwnerCreditsDepleted => "workspace_owner_credits_depleted",
    MemberCreditsDepleted => "workspace_member_credits_depleted",
    Usage => "rate_limit_reached",
    OwnerUsage => "workspace_owner_usage_limit_reached",
    MemberUsage => "workspace_member_usage_limit_reached",
});
wire_enum!(ClaudeLimitStatus {
    Rejected => "rejected"
});
wire_enum!(ClaudeLimitWindow {
    FiveHour => "five_hour",
    SevenDay => "seven_day",
    SevenDayOpus => "seven_day_opus",
    SevenDaySonnet => "seven_day_sonnet",
    Overage => "overage",
});

#[derive(Clone, Debug)]
pub(in crate::runtime) struct Signal {
    pub key: Option<crate::state::KeyAuthority>,
    pub state: AvailabilityState,
    pub until: Option<u64>,
    pub reset_floor: Option<u64>,
    pub window: Option<String>,
    pub reason: AuthReason,
    pub turn_error: bool,
    pub rejected_resets: Vec<Value>,
}
impl Signal {
    pub fn limited(reason: AuthReason) -> Self {
        Self {
            key: None,
            state: AvailabilityState::Limited,
            until: None,
            reset_floor: None,
            window: None,
            reason,
            turn_error: false,
            rejected_resets: vec![],
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::runtime) struct CodexEnvelope {
    pub rate_limits: CodexLimits,
}

/// Codex 0.161.0 exported `GetAccountRateLimitsResponse` contract. This permission
/// exists only on a live read, never on a sparse notification.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::runtime) struct CodexRead {
    pub ordinary_usage_allowed: Option<bool>,
    pub rate_limits: CodexLimits,
}
impl CodexRead {
    pub fn confirms_usage(&self) -> bool {
        self.ordinary_usage_allowed == Some(true)
            && self.rate_limits.spend_control_reached != Some(true)
            && !self.rate_limits.spending_blocked()
            && [&self.rate_limits.primary, &self.rate_limits.secondary]
                .into_iter()
                .all(|w| {
                    w.as_ref().is_some_and(|w| {
                        w.used_percent.is_finite()
                            && (0.0..EXHAUSTED_PERCENT).contains(&w.used_percent)
                    })
                })
    }
}

pub(in crate::runtime) fn codex_error(raw: &Value) -> Option<Signal> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ErrorInfo {
        codex_error_info: CodexErrorInfo,
    }
    let info: ErrorInfo = serde_json::from_value(raw.clone())
        .inspect_err(|error| {
            crate::runtime::provider::protocol::drift("codex_error_info", Some(error));
        })
        .ok()?;
    let mut signal = match info.codex_error_info {
        CodexErrorInfo::UsageLimitExceeded => Signal::limited(AuthReason::UsageLimit),
        CodexErrorInfo::Unauthorized => Signal::limited(AuthReason::Authentication),
        CodexErrorInfo::BillingError => Signal {
            state: AvailabilityState::Unavailable,
            ..Signal::limited(AuthReason::SpendCap)
        },
        CodexErrorInfo::Other => return None,
    };
    signal.turn_error = true;
    Some(signal)
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::runtime) struct CodexLimits {
    pub primary: Option<Window>,
    pub secondary: Option<Window>,
    pub spend_control_reached: Option<bool>,
    pub rate_limit_reached_type: Option<LimitReached>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::runtime) struct Window {
    pub used_percent: f64,
    // Normalize once when ingested, not whenever retained snapshots are evaluated.
    #[serde(default)]
    pub resets_at: Value,
    #[serde(skip)]
    rejected_reset: Option<Value>,
}
struct ResetTime {
    at: Option<u64>,
    rejected: Option<Value>,
}
fn normalize(raw: &Value, now: u64) -> ResetTime {
    if raw.is_null() {
        return ResetTime {
            at: None,
            rejected: None,
        };
    }
    if raw.as_i64().is_some_and(|v| v <= 0) {
        return ResetTime {
            at: Some(0),
            rejected: None,
        };
    }
    if let Some(at) = raw.as_u64() {
        if at <= now.saturating_add(MAX_RESET_DELAY_SECONDS) {
            return ResetTime {
                at: Some(at),
                rejected: None,
            };
        }
        return ResetTime {
            at: None,
            rejected: Some(raw.clone()),
        };
    }
    // Do not persist arbitrary strings from native frames.
    ResetTime {
        at: None,
        rejected: Some(if raw.is_number() {
            raw.clone()
        } else {
            Value::String("invalid_type".into())
        }),
    }
}
impl CodexLimits {
    pub fn merge(&mut self, next: Self) {
        self.merge_at(next, crate::state::now());
    }
    pub fn merge_at(&mut self, next: Self, now: u64) {
        let normalize_window = |mut window: Window| {
            let ResetTime { at, rejected } = normalize(&window.resets_at, now);
            window.resets_at = at.map_or(Value::Null, Value::from);
            window.rejected_reset = rejected;
            window
        };
        if let Some(window) = next.primary {
            self.primary = Some(normalize_window(window));
        }
        if let Some(window) = next.secondary {
            self.secondary = Some(normalize_window(window));
        }
        // Upstream explicitly defines null sparse account metadata as unavailable.
        if next.spend_control_reached.is_some() {
            self.spend_control_reached = next.spend_control_reached;
        }
        if next.rate_limit_reached_type.is_some() {
            self.rate_limit_reached_type = next.rate_limit_reached_type;
        }
    }
    fn spending_blocked(&self) -> bool {
        self.spend_control_reached == Some(true)
            || matches!(
                self.rate_limit_reached_type,
                Some(LimitReached::OwnerCreditsDepleted | LimitReached::MemberCreditsDepleted)
            )
    }
    pub fn signal(&self) -> Option<Signal> {
        self.signal_at(crate::state::now())
    }
    pub fn signal_at(&self, now: u64) -> Option<Signal> {
        if self.spending_blocked() {
            return Some(Signal {
                state: AvailabilityState::Unavailable,
                ..Signal::limited(AuthReason::SpendCap)
            });
        }
        if !matches!(
            self.rate_limit_reached_type,
            Some(LimitReached::Usage | LimitReached::OwnerUsage | LimitReached::MemberUsage)
        ) {
            return None;
        }
        let signal = self.exhaustion(now);
        // Healthy or elapsed populated windows cannot resurrect a retained reached marker.
        if signal.window.is_none() && (self.primary.is_some() || self.secondary.is_some()) {
            return None;
        }
        Some(signal)
    }
    pub fn exhaustion(&self, now: u64) -> Signal {
        let exhausted: Vec<_> = [("primary", &self.primary), ("secondary", &self.secondary)]
            .into_iter()
            .filter_map(|(name, w)| {
                w.as_ref()
                    .filter(|w| {
                        w.used_percent >= EXHAUSTED_PERCENT
                            && w.resets_at.as_u64().is_none_or(|at| at > now)
                    })
                    .map(|w| (name, w))
            })
            .collect();
        let floor = exhausted
            .iter()
            .filter_map(|(_, w)| w.resets_at.as_u64())
            .max();
        let known = !exhausted.is_empty()
            && exhausted
                .iter()
                .all(|(_, w)| w.resets_at.as_u64().is_some());
        Signal {
            until: known.then_some(floor).flatten(),
            reset_floor: floor,
            window: (!exhausted.is_empty()).then(|| {
                exhausted
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join("+")
            }),
            rejected_resets: exhausted
                .iter()
                .filter_map(|(_, w)| w.rejected_reset.clone())
                .collect(),
            ..Signal::limited(AuthReason::UsageLimit)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::runtime) struct ClaudeLimits {
    pub status: ClaudeLimitStatus,
    #[serde(default)]
    pub resets_at: Value,
    pub rate_limit_type: Option<String>,
}
impl ClaudeLimits {
    pub fn signal(&self) -> Option<Signal> {
        if self.status != ClaudeLimitStatus::Rejected {
            return None;
        }
        let now = crate::state::now();
        let ResetTime {
            at: until,
            rejected,
        } = normalize(&self.resets_at, now);
        if until.is_some_and(|at| at <= now) {
            return None;
        }
        Some(Signal {
            until,
            reset_floor: until,
            window: self
                .rate_limit_type
                .as_ref()
                .filter(|s| ClaudeLimitWindow::parse(s) != ClaudeLimitWindow::Other)
                .cloned(),
            rejected_resets: rejected.into_iter().collect(),
            ..Signal::limited(AuthReason::UsageLimit)
        })
    }
}
