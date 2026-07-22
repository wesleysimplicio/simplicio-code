//! Observable admission and routing for an externally invoking LLM.
//!
//! This module never calls a provider and never selects a local/internal model. It evaluates
//! provider routes supplied by the invoking authority and returns an auditable recommendation
//! that the caller may accept according to its own policy.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const EXTERNAL_ROUTING_SCHEMA: &str = "simplicio.external-routing/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPolicy {
    Interactive,
    Background,
    Review,
    Delivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementState {
    Measured,
    Estimated,
    Missing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation<T> {
    pub state: MeasurementState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
impl<T> Observation<T> {
    pub fn measured(value: T) -> Self {
        Self {
            state: MeasurementState::Measured,
            value: Some(value),
            reason: None,
        }
    }
    pub fn estimated(value: T) -> Self {
        Self {
            state: MeasurementState::Estimated,
            value: Some(value),
            reason: None,
        }
    }
    pub fn missing(reason: impl Into<String>) -> Self {
        Self {
            state: MeasurementState::Missing,
            value: None,
            reason: Some(reason.into()),
        }
    }
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            state: MeasurementState::Failed,
            value: None,
            reason: Some(reason.into()),
        }
    }
    fn usable(&self) -> Option<&T> {
        self.value.as_ref().filter(|_| {
            matches!(
                self.state,
                MeasurementState::Measured | MeasurementState::Estimated
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BudgetScope {
    pub organization: String,
    pub project: String,
    pub session: String,
    pub turn: String,
    pub stage: String,
    pub agent: String,
}
impl BudgetScope {
    fn validate(&self) -> Result<(), RoutingError> {
        for (name, value) in [
            ("organization", &self.organization),
            ("project", &self.project),
            ("session", &self.session),
            ("turn", &self.turn),
            ("stage", &self.stage),
            ("agent", &self.agent),
        ] {
            if value.trim().is_empty() {
                return Err(RoutingError::Invalid(format!("{name} is empty")));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub currency: String,
    pub micros: u64,
}
impl Money {
    pub fn new(currency: impl Into<String>, micros: u64) -> Result<Self, RoutingError> {
        let currency = currency.into().to_ascii_uppercase();
        if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(RoutingError::Invalid(
                "currency must be a three-letter ISO-style code".into(),
            ));
        }
        Ok(Self { currency, micros })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLimit {
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_context_bytes: Option<u64>,
    pub max_cost: Option<Money>,
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_bytes: u64,
    pub costs: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSignals {
    pub quota_remaining_tokens: Observation<u64>,
    pub latency_ms: Observation<u64>,
    pub healthy: Observation<bool>,
    pub input_tokens: Observation<u64>,
    pub output_tokens: Observation<u64>,
    pub context_bytes: Observation<u64>,
    pub cache_hit: Observation<bool>,
    pub cost: Observation<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalRoute {
    /// Opaque route identifier chosen by the external invoking authority.
    pub route_id: String,
    pub provider: String,
    /// Must be true. Internal/local inference is rejected rather than used as fallback.
    pub externally_authorized: bool,
    pub signals: ProviderSignals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRequest {
    pub dispatch_id: String,
    pub scope: BudgetScope,
    pub policy: WorkPolicy,
    pub candidates: Vec<ExternalRoute>,
    pub expected_input_tokens: u64,
    pub expected_output_tokens: u64,
    pub context_bytes: u64,
    pub fan_out: u32,
    pub effect_reconciled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteExplanation {
    pub schema: String,
    pub dispatch_id: String,
    pub policy: WorkPolicy,
    pub selected_route_id: Option<String>,
    pub selected_provider: Option<String>,
    pub reason: String,
    pub signals_used: Vec<String>,
    pub admitted: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RoutingError {
    #[error("invalid routing request: {0}")]
    Invalid(String),
    #[error("dispatch denied: {0}")]
    Denied(String),
    #[error("dispatch {0} already has an unreconciled effect")]
    EffectUnknown(String),
    #[error("accounting conflict for dispatch {0}")]
    AccountingConflict(String),
}

#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub max_fan_out: u32,
    pub max_active: u32,
    pub interactive_reserved: u32,
}

pub struct ExternalRouter {
    config: RouterConfig,
    limits: HashMap<BudgetScope, BudgetLimit>,
    usage: HashMap<BudgetScope, Usage>,
    active_interactive: u32,
    active_background: u32,
    effects: HashSet<String>,
    reconciled: HashMap<String, (BudgetScope, Usage, WorkPolicy)>,
}
impl ExternalRouter {
    pub fn new(config: RouterConfig) -> Result<Self, RoutingError> {
        if config.max_fan_out == 0
            || config.max_active == 0
            || config.interactive_reserved == 0
            || config.interactive_reserved > config.max_active
        {
            return Err(RoutingError::Invalid(
                "capacity bounds must be non-zero and interactive_reserved <= max_active".into(),
            ));
        }
        Ok(Self {
            config,
            limits: HashMap::new(),
            usage: HashMap::new(),
            active_interactive: 0,
            active_background: 0,
            effects: HashSet::new(),
            reconciled: HashMap::new(),
        })
    }
    pub fn set_budget(
        &mut self,
        scope: BudgetScope,
        limit: BudgetLimit,
    ) -> Result<(), RoutingError> {
        scope.validate()?;
        self.limits.insert(scope, limit);
        Ok(())
    }
    pub fn usage(&self, scope: &BudgetScope) -> Usage {
        self.usage.get(scope).cloned().unwrap_or_default()
    }

    pub fn recommend(
        &mut self,
        request: &DispatchRequest,
    ) -> Result<RouteExplanation, RoutingError> {
        request.scope.validate()?;
        if request.dispatch_id.trim().is_empty() {
            return Err(RoutingError::Invalid("dispatch_id is empty".into()));
        }
        if self.effects.contains(&request.dispatch_id) && !request.effect_reconciled {
            return Err(RoutingError::EffectUnknown(request.dispatch_id.clone()));
        }
        if request.fan_out == 0 || request.fan_out > self.config.max_fan_out {
            return self.denied(request, "fan-out cap exceeded");
        }
        self.check_capacity(request)?;
        self.check_budget(request)?;
        let mut eligible: Vec<&ExternalRoute> = request
            .candidates
            .iter()
            .filter(|r| {
                r.externally_authorized
                    && !r.route_id.trim().is_empty()
                    && r.signals.healthy.usable().copied().unwrap_or(true)
                    && r.signals
                        .quota_remaining_tokens
                        .usable()
                        .copied()
                        .is_none_or(|q| {
                            q >= request
                                .expected_input_tokens
                                .saturating_add(request.expected_output_tokens)
                        })
            })
            .collect();
        if eligible.is_empty() {
            return self.denied(
                request,
                "no externally authorized route has usable quota/health",
            );
        }
        eligible.sort_by_key(|route| {
            (
                route
                    .signals
                    .latency_ms
                    .usable()
                    .copied()
                    .unwrap_or(u64::MAX),
                route
                    .signals
                    .cost
                    .usable()
                    .map(|m| m.micros)
                    .unwrap_or(u64::MAX),
                route.route_id.as_str(),
            )
        });
        let selected = eligible[0];
        if let (Some(limit), Some(cost)) = (
            self.limits
                .get(&request.scope)
                .and_then(|limit| limit.max_cost.as_ref()),
            selected.signals.cost.usable(),
        ) {
            if limit.currency != cost.currency {
                return Err(RoutingError::Denied(
                    "route cost currency does not match the declared budget".into(),
                ));
            }
            let used = self
                .usage(&request.scope)
                .costs
                .get(&limit.currency)
                .copied()
                .unwrap_or(0);
            if used.saturating_add(cost.micros) > limit.micros {
                return Err(RoutingError::Denied(
                    "declared cost budget exhausted".into(),
                ));
            }
        }
        self.effects.insert(request.dispatch_id.clone());
        match request.policy {
            WorkPolicy::Interactive => self.active_interactive += 1,
            _ => self.active_background += 1,
        }
        Ok(RouteExplanation { schema: EXTERNAL_ROUTING_SCHEMA.into(), dispatch_id: request.dispatch_id.clone(), policy: request.policy, selected_route_id: Some(selected.route_id.clone()), selected_provider: Some(selected.provider.clone()), reason: "lowest observed latency, then cost, among externally authorized healthy routes within quota and budget".into(), signals_used: vec!["external_authority".into(), "quota".into(), "health".into(), "latency".into(), "cost".into(), "capacity".into(), "budget".into()], admitted: true })
    }

    fn denied(&self, r: &DispatchRequest, reason: &str) -> Result<RouteExplanation, RoutingError> {
        Ok(RouteExplanation {
            schema: EXTERNAL_ROUTING_SCHEMA.into(),
            dispatch_id: r.dispatch_id.clone(),
            policy: r.policy,
            selected_route_id: None,
            selected_provider: None,
            reason: reason.into(),
            signals_used: vec![
                "external_authority".into(),
                "quota".into(),
                "health".into(),
                "capacity".into(),
                "budget".into(),
            ],
            admitted: false,
        })
    }
    fn check_capacity(&self, r: &DispatchRequest) -> Result<(), RoutingError> {
        let total = self.active_interactive + self.active_background;
        if total >= self.config.max_active {
            return Err(RoutingError::Denied(
                "backpressure: active capacity exhausted".into(),
            ));
        }
        if r.policy != WorkPolicy::Interactive
            && self.active_background >= self.config.max_active - self.config.interactive_reserved
        {
            return Err(RoutingError::Denied(
                "backpressure: capacity reserved for interactive work".into(),
            ));
        }
        Ok(())
    }
    fn check_budget(&self, r: &DispatchRequest) -> Result<(), RoutingError> {
        let Some(limit) = self.limits.get(&r.scope) else {
            return Ok(());
        };
        let used = self.usage(&r.scope);
        if limit
            .max_input_tokens
            .is_some_and(|v| used.input_tokens.saturating_add(r.expected_input_tokens) > v)
            || limit
                .max_output_tokens
                .is_some_and(|v| used.output_tokens.saturating_add(r.expected_output_tokens) > v)
            || limit
                .max_context_bytes
                .is_some_and(|v| used.context_bytes.saturating_add(r.context_bytes) > v)
        {
            return Err(RoutingError::Denied(
                "declared token/context budget exhausted".into(),
            ));
        }
        Ok(())
    }

    /// Reconciles the provider receipt exactly once. A retry with identical data is idempotent;
    /// conflicting data fails closed, so retry/cancel cannot double-charge an effect.
    pub fn reconcile(
        &mut self,
        dispatch_id: &str,
        scope: &BudgetScope,
        actual: Usage,
        policy: WorkPolicy,
    ) -> Result<bool, RoutingError> {
        if let Some(previous) = self.reconciled.get(dispatch_id) {
            return if previous == &(scope.clone(), actual.clone(), policy) {
                Ok(false)
            } else {
                Err(RoutingError::AccountingConflict(dispatch_id.into()))
            };
        }
        if !self.effects.remove(dispatch_id) {
            return Ok(false);
        }
        let receipt = actual.clone();
        let entry = self.usage.entry(scope.clone()).or_default();
        entry.input_tokens = entry
            .input_tokens
            .checked_add(actual.input_tokens)
            .ok_or_else(|| RoutingError::AccountingConflict(dispatch_id.into()))?;
        entry.output_tokens = entry
            .output_tokens
            .checked_add(actual.output_tokens)
            .ok_or_else(|| RoutingError::AccountingConflict(dispatch_id.into()))?;
        entry.context_bytes = entry
            .context_bytes
            .checked_add(actual.context_bytes)
            .ok_or_else(|| RoutingError::AccountingConflict(dispatch_id.into()))?;
        for (currency, micros) in actual.costs {
            *entry.costs.entry(currency).or_default() = entry
                .costs
                .get(&currency)
                .copied()
                .unwrap_or(0)
                .checked_add(micros)
                .ok_or_else(|| RoutingError::AccountingConflict(dispatch_id.into()))?;
        }
        match policy {
            WorkPolicy::Interactive => {
                self.active_interactive = self.active_interactive.saturating_sub(1)
            }
            _ => self.active_background = self.active_background.saturating_sub(1),
        }
        self.reconciled
            .insert(dispatch_id.into(), (scope.clone(), receipt, policy));
        Ok(true)
    }
    pub fn cancel_before_effect(&mut self, dispatch_id: &str, policy: WorkPolicy) -> bool {
        if !self.effects.remove(dispatch_id) {
            return false;
        }
        match policy {
            WorkPolicy::Interactive => {
                self.active_interactive = self.active_interactive.saturating_sub(1)
            }
            _ => self.active_background = self.active_background.saturating_sub(1),
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPack {
    pub hash: String,
    pub provenance: Vec<String>,
    pub bytes: u64,
}
#[derive(Default)]
pub struct ContextPackRegistry {
    packs: HashMap<String, ContextPack>,
}
impl ContextPackRegistry {
    pub fn insert(
        &mut self,
        content: &[u8],
        mut provenance: Vec<String>,
    ) -> Result<(ContextPack, bool), RoutingError> {
        if provenance.is_empty() || provenance.iter().any(|p| p.trim().is_empty()) {
            return Err(RoutingError::Invalid(
                "context provenance is required".into(),
            ));
        }
        provenance.sort();
        provenance.dedup();
        let hash = blake3::hash(content).to_hex().to_string();
        if let Some(existing) = self.packs.get_mut(&hash) {
            existing.provenance.extend(provenance);
            existing.provenance.sort();
            existing.provenance.dedup();
            return Ok((existing.clone(), true));
        }
        let pack = ContextPack {
            hash: hash.clone(),
            provenance,
            bytes: content.len() as u64,
        };
        self.packs.insert(hash, pack.clone());
        Ok((pack, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    fn scope() -> BudgetScope {
        BudgetScope {
            organization: "o".into(),
            project: "p".into(),
            session: "s".into(),
            turn: "t".into(),
            stage: "build".into(),
            agent: "a".into(),
        }
    }
    fn signals(quota: u64, latency: u64) -> ProviderSignals {
        ProviderSignals {
            quota_remaining_tokens: Observation::measured(quota),
            latency_ms: Observation::measured(latency),
            healthy: Observation::measured(true),
            input_tokens: Observation::estimated(10),
            output_tokens: Observation::missing("provider omitted usage"),
            context_bytes: Observation::measured(100),
            cache_hit: Observation::missing("unsupported"),
            cost: Observation::missing("provider omitted price"),
        }
    }
    fn request(id: &str, policy: WorkPolicy) -> DispatchRequest {
        DispatchRequest {
            dispatch_id: id.into(),
            scope: scope(),
            policy,
            candidates: vec![ExternalRoute {
                route_id: "external-fast".into(),
                provider: "test-provider".into(),
                externally_authorized: true,
                signals: signals(100, 10),
            }],
            expected_input_tokens: 10,
            expected_output_tokens: 10,
            context_bytes: 100,
            fan_out: 1,
            effect_reconciled: false,
        }
    }
    fn router() -> ExternalRouter {
        ExternalRouter::new(RouterConfig {
            max_fan_out: 20,
            max_active: 20,
            interactive_reserved: 2,
        })
        .unwrap()
    }
    #[test]
    fn never_falls_back_to_internal_or_quota_exhausted_route() {
        let mut r = router();
        let mut q = request("d", WorkPolicy::Interactive);
        q.candidates[0].externally_authorized = false;
        assert!(!r.recommend(&q).unwrap().admitted);
        q.candidates[0].externally_authorized = true;
        q.candidates[0].signals.quota_remaining_tokens = Observation::measured(1);
        assert!(!r.recommend(&q).unwrap().admitted);
    }
    #[test]
    fn reserves_interactive_capacity_under_background_load() {
        let mut r = ExternalRouter::new(RouterConfig {
            max_fan_out: 1,
            max_active: 3,
            interactive_reserved: 1,
        })
        .unwrap();
        assert!(
            r.recommend(&request("b1", WorkPolicy::Background))
                .unwrap()
                .admitted
        );
        assert!(
            r.recommend(&request("b2", WorkPolicy::Background))
                .unwrap()
                .admitted
        );
        assert!(matches!(
            r.recommend(&request("b3", WorkPolicy::Background)),
            Err(RoutingError::Denied(_))
        ));
        assert!(
            r.recommend(&request("i", WorkPolicy::Interactive))
                .unwrap()
                .admitted
        );
    }
    #[test]
    fn retry_and_cancel_are_exactly_once() {
        let mut r = router();
        let q = request("d", WorkPolicy::Delivery);
        r.recommend(&q).unwrap();
        let mut u = Usage::default();
        u.input_tokens = 7;
        u.costs.insert("USD".into(), 3);
        assert!(r.reconcile("d", &q.scope, u.clone(), q.policy).unwrap());
        assert!(!r.reconcile("d", &q.scope, u, q.policy).unwrap());
        assert_eq!(r.usage(&q.scope).input_tokens, 7);
        assert!(matches!(
            r.reconcile("d", &q.scope, Usage::default(), q.policy),
            Err(RoutingError::AccountingConflict(_))
        ));
        assert!(!r.cancel_before_effect("d", q.policy));
    }
    #[test]
    fn context_is_content_addressed_and_merges_provenance() {
        let mut r = ContextPackRegistry::default();
        let (a, reused) = r.insert(b"shared", vec!["mapper:abc".into()]).unwrap();
        assert!(!reused);
        let (b, reused) = r.insert(b"shared", vec!["artifact:def".into()]).unwrap();
        assert!(reused);
        assert_eq!(a.hash, b.hash);
        assert_eq!(b.provenance, vec!["artifact:def", "mapper:abc"]);
    }
    #[test]
    fn observations_never_invent_missing_values_or_cost() {
        let x: Observation<Money> = Observation::missing("no pricing header");
        let json = serde_json::to_value(x).unwrap();
        assert!(json["value"].is_null());
        assert_eq!(json["state"], "missing");
    }

    #[test]
    fn twenty_agents_low_quota_slow_provider_and_shared_context() {
        let mut router = ExternalRouter::new(RouterConfig {
            max_fan_out: 20,
            max_active: 20,
            interactive_reserved: 2,
        })
        .unwrap();
        let mut contexts = ContextPackRegistry::default();
        assert!(
            !contexts
                .insert(b"one mapper context", vec!["mapper:sha256".into()])
                .unwrap()
                .1
        );
        for agent in 0..20 {
            let mut request = request(
                &format!("agent-{agent}"),
                if agent == 19 {
                    WorkPolicy::Interactive
                } else {
                    WorkPolicy::Background
                },
            );
            request.candidates[0].signals.latency_ms = Observation::measured(5_000);
            request.candidates[0].signals.quota_remaining_tokens =
                Observation::measured(if agent < 4 { 100 } else { 1 });
            let decision = router.recommend(&request).unwrap();
            assert_eq!(decision.admitted, agent < 4);
            assert!(
                contexts
                    .insert(b"one mapper context", vec![format!("agent:{agent}")])
                    .unwrap()
                    .1
            );
            if decision.admitted {
                assert!(router.cancel_before_effect(&request.dispatch_id, request.policy));
            }
        }
    }
    proptest! { #[test] fn budget_never_admits_over_cap(cap in 0u64..10_000, used in 0u64..10_000, requested in 0u64..10_000) { let mut r=router(); let s=scope(); r.set_budget(s.clone(),BudgetLimit { max_input_tokens:Some(cap),max_output_tokens:None,max_context_bytes:None,max_cost:None }).unwrap(); r.usage.insert(s.clone(),Usage { input_tokens:used,..Usage::default() }); let mut q=request("p",WorkPolicy::Review); q.expected_input_tokens=requested; let admitted=r.recommend(&q).map(|d|d.admitted).unwrap_or(false); prop_assert!(!admitted || used.saturating_add(requested)<=cap); } }
}
