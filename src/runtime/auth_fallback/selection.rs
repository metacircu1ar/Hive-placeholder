//! Profile candidate selection from validated availability and billing facts.
use super::{AuthChoice, Availability};
use crate::{
    config::{AuthGroup, Provider},
    error::{Error, Result},
    runtime::{core::preflight::MemberFacts, store::Queries},
};

pub(in crate::runtime) struct Selection {
    pub index: usize,
    pub availability: SelectedAvailability,
    pub skipped: Vec<String>,
    /// Why this member was chosen; group callers record it as evidence.
    pub choice: AuthChoice,
}

pub(in crate::runtime) enum SelectedAvailability {
    Ready,
    Waiting { until: Option<u64> },
}
impl Selection {
    pub fn waiting(&self) -> bool {
        matches!(self.availability, SelectedAvailability::Waiting { .. })
    }
    pub fn deadline(&self) -> Option<u64> {
        match self.availability {
            SelectedAvailability::Ready => None,
            SelectedAvailability::Waiting { until } => until,
        }
    }
}

pub(in crate::runtime) struct SelectionRequest<'a> {
    pub queries: &'a Queries,
    pub members: &'a [MemberFacts],
    pub model: &'a str,
    pub effort: &'a str,
    pub follow_availability: bool,
    pub allow_api: bool,
    pub sticky: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub group: Option<&'a AuthGroup>,
    pub capacity: Option<Capacity<'a>>,
}

pub(in crate::runtime) struct Capacity<'a> {
    pub provider: Provider,
    pub desired: &'a crate::options::Snapshot,
    pub reserved: Option<&'a crate::options::Snapshot>,
    pub limit: usize,
}
#[derive(Default)]
struct Candidates<'a> {
    compatible: Vec<(usize, Availability)>,
    skipped: Vec<String>,
    inconclusive: Option<&'a Error>,
    model_seen: bool,
}

/// Proof that every candidate was inspected without an inconclusive discovery error.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub(in crate::runtime) enum CatalogMiss {
    #[error("No candidate catalog contains the requested model.")]
    Model,
    #[error("No candidate catalog supports the requested effort.")]
    Effort,
}

/// Process-scope keys for balanced admission, computed once per selection.
struct Admission {
    desired: String,
    reserved: Option<String>,
    limit: usize,
}
impl Admission {
    fn new(capacity: &Capacity<'_>) -> Result<Self> {
        Ok(Self {
            desired: capacity.desired.process_key(capacity.provider)?,
            reserved: capacity
                .reserved
                .map(|snapshot| snapshot.process_key(capacity.provider))
                .transpose()?,
            limit: capacity.limit,
        })
    }
    fn admits(&self, queries: &Queries, auth: &str, agent: Option<&str>) -> Result<bool> {
        queries.admits_process_keys(
            auth,
            agent,
            &self.desired,
            self.reserved.as_deref(),
            self.limit,
        )
    }
}

/// Member order, plus load and process-variant admission in balanced groups.
struct Chooser<'a> {
    queries: &'a Queries,
    members: &'a [MemberFacts],
    agent: Option<&'a str>,
    balanced: Option<&'a AuthGroup>,
    admission: Option<Admission>,
}
impl Chooser<'_> {
    fn api(&self, index: usize) -> bool {
        self.members[index].authority().is_some()
    }
    fn mode(&self) -> AuthChoice {
        if self.balanced.is_some() {
            AuthChoice::Balanced
        } else {
            AuthChoice::Ordered
        }
    }
    /// Balanced choices skip subscriptions whose logical process variants are
    /// full; ordered groups and manual switches keep the later destination check.
    fn admitted(&self, candidates: impl IntoIterator<Item = usize>) -> Result<(Vec<usize>, bool)> {
        let mut admitted = Vec::new();
        let mut refused = false;
        for index in candidates {
            let admits = match &self.admission {
                Some(admission) if !self.api(index) => {
                    admission.admits(self.queries, &self.members[index].auth, self.agent)?
                }
                _ => true,
            };
            if admits {
                admitted.push(index);
            } else {
                refused = true;
            }
        }
        Ok((admitted, refused))
    }
    /// The first subscription in member order or, in a balanced group, the
    /// minimum `(bound agents + active turns + 1) / weight`. Fractions are
    /// compared exactly by cross-multiplication; ties keep member order.
    fn subscription(&self, candidates: impl IntoIterator<Item = usize>) -> Result<Option<usize>> {
        let mut best: Option<(usize, u128, u128)> = None;
        for index in candidates.into_iter().filter(|index| !self.api(*index)) {
            let Some(group) = self.balanced else {
                return Ok(Some(index));
            };
            let auth = &self.members[index].auth;
            let weight = u128::from(group.weight(auth));
            let load = self.queries.auth_load(auth)?;
            let score = u128::from(load.bound_agents) + u128::from(load.active_turns) + 1;
            if best.is_none_or(|(_, old_score, old_weight)| score * old_weight < old_score * weight)
            {
                best = Some((index, score, weight));
            }
        }
        Ok(best.map(|(index, ..)| index))
    }
    /// A confirmed plan replaces a sticky API key; otherwise a usable current
    /// login stays, then subscriptions precede the ordered paid tail.
    fn ready(
        &self,
        ready: &[usize],
        sticky: Option<&str>,
        sticky_api: bool,
    ) -> Result<Option<(usize, AuthChoice)>> {
        if sticky_api && let Some(index) = self.subscription(ready.iter().copied())? {
            return Ok(Some((index, self.mode())));
        }
        if let Some(index) = ready
            .iter()
            .copied()
            .find(|index| sticky == Some(self.members[*index].auth.as_str()))
        {
            return Ok(Some((index, AuthChoice::Sticky)));
        }
        if let Some(index) = self.subscription(ready.iter().copied())? {
            return Ok(Some((index, self.mode())));
        }
        Ok(ready.first().map(|index| (*index, AuthChoice::PaidTail)))
    }
    /// Balanced groups spread waiting spawns like ready ones, so a shared reset
    /// does not start every waiter on the first member.
    fn waiting(
        &self,
        compatible: &[(usize, Availability)],
        allow_api: bool,
    ) -> Result<Option<usize>> {
        let indices = || compatible.iter().map(|(index, _)| *index);
        if self.balanced.is_some() {
            let (admitted, refused) = self.admitted(indices().filter(|index| !self.api(*index)))?;
            if let Some(index) = self.subscription(admitted)? {
                return Ok(Some(index));
            }
            if refused {
                return Err(no_capacity());
            }
        }
        Ok(indices().find(|index| !self.api(*index) || allow_api))
    }
}

fn no_capacity() -> Error {
    Error::process_variant_limit(
        "No balanced login has capacity for these process settings; resume after \
            capacity changes.",
    )
}

pub(in crate::runtime) fn select(request: SelectionRequest<'_>) -> Result<Selection> {
    let SelectionRequest {
        queries,
        members,
        model,
        effort,
        follow_availability,
        allow_api,
        sticky,
        agent,
        group,
        capacity,
    } = request;
    let Candidates {
        compatible,
        skipped,
        inconclusive,
        model_seen,
    } = candidates(queries, members, model, effort)?;
    let balanced = group.filter(|group| group.balanced());
    let chooser = Chooser {
        queries,
        members,
        agent,
        balanced,
        admission: balanced
            .and(capacity)
            .map(|capacity| Admission::new(&capacity))
            .transpose()?,
    };
    let now = crate::state::now();
    let is_api = |index: usize| chooser.api(index);
    let sticky_api = members
        .iter()
        .position(|m| Some(m.auth.as_str()) == sticky)
        .is_some_and(is_api);
    // Billing changes require positive exhaustion evidence from every plan.
    // A missing login/catalog or authentication failure never implies exhaustion.
    let plans_exhausted = compatible.iter().any(|(index, _)| is_api(*index))
        && inconclusive.is_none()
        && exhausted_plans(queries, members, now, agent)?;
    let available = |(index, value): &&(usize, Availability)| {
        (!is_api(*index) || (allow_api && (!follow_availability || sticky_api || plans_exhausted)))
            && (!follow_availability || value.usable(now, agent))
            && (!sticky_api || is_api(*index) || value.confirmed_at.is_some())
    };
    let (ready, capacity_refused) =
        chooser.admitted(compatible.iter().filter(available).map(|(index, _)| *index))?;
    if let Some((index, choice)) = chooser.ready(&ready, sticky, sticky_api)? {
        return Ok(Selection {
            index,
            availability: SelectedAvailability::Ready,
            skipped,
            choice,
        });
    }
    if let Some(error) = inconclusive {
        return Err(error.clone());
    }
    if capacity_refused {
        return Err(no_capacity());
    }
    if let Some(waiting) = waiting_selection(&chooser, &compatible, allow_api, now, skipped)? {
        return Ok(waiting);
    }
    Err(if model_seen {
        Error::unsupported_effort("No login supports the requested model effort.")
            .with_source(CatalogMiss::Effort)
    } else {
        Error::unknown_model("No login has the requested model in its catalog.")
            .with_source(CatalogMiss::Model)
    })
}

fn exhausted_plans(
    queries: &Queries,
    members: &[MemberFacts],
    now: u64,
    agent: Option<&str>,
) -> Result<bool> {
    for member in members {
        let binding = member.binding.as_ref().map_err(Clone::clone)?;
        if binding.profile.method == crate::config::Method::Subscription {
            let value = queries.availability(&member.auth, binding.id)?;
            if value.state != super::AvailabilityState::Limited
                || value.reason != Some(super::AuthReason::UsageLimit)
                || value.usable(now, agent)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

impl<'a> Candidates<'a> {
    fn defer(mut self, error: &'a Error) -> Self {
        if self.inconclusive.is_none()
            || crate::runtime::provider::catalogs::capacity_exhausted(error)
        {
            self.inconclusive = Some(error);
        }
        self
    }
}

fn candidates<'a>(
    queries: &Queries,
    members: &'a [MemberFacts],
    model: &str,
    effort: &str,
) -> Result<Candidates<'a>> {
    // try_fold retains query order and stops at the first hard error; inconclusive
    // bindings/catalogs remain evidence until all usable alternatives are considered.
    members
        .iter()
        .enumerate()
        .try_fold(Candidates::default(), |mut candidates, (index, member)| {
            let binding = match &member.binding {
                Ok(value) => value,
                Err(error) => return Ok(candidates.defer(error)),
            };
            let catalog = match &member.catalog {
                Some(Ok(value)) => value,
                Some(Err(error)) => return Ok(candidates.defer(error)),
                None => return Err(Error::internal_error("Missing spawn catalog preflight.")),
            };
            let selected = catalog.iter().find(|m| m.name() == Some(model));
            candidates.model_seen |= selected.is_some();
            if selected.is_some_and(|m| m.supports_effort(effort)) {
                let mut value = queries.availability(&member.auth, binding.id)?;
                if value.key.as_ref() != member.authority() {
                    if member.authority().is_some_and(|key| {
                        value
                            .key
                            .as_ref()
                            .is_none_or(|old| key.revision > old.revision)
                    }) {
                        value = Availability::available(&member.auth, binding.id);
                        value.key = member.authority().cloned();
                    } else {
                        return Err(Error::agent_busy(
                            "API key catalog proof was superseded; retry.",
                        ));
                    }
                }
                candidates.compatible.push((index, value));
            } else {
                candidates.skipped.push(member.auth.clone());
            }
            Ok(candidates)
        })
}

fn waiting_selection(
    chooser: &Chooser<'_>,
    compatible: &[(usize, Availability)],
    allow_api: bool,
    now: u64,
    skipped: Vec<String>,
) -> Result<Option<Selection>> {
    if let Some(index) = chooser.waiting(compatible, allow_api)? {
        return Ok(Some(Selection {
            index,
            availability: SelectedAvailability::Waiting {
                until: compatible
                    .iter()
                    .filter_map(|(_, a)| a.deadline())
                    .filter(|at| *at > now)
                    .min(),
            },
            skipped,
            choice: AuthChoice::Waiting,
        }));
    }
    Ok(None)
}
