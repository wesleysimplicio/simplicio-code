//! Read-only Agent Fabric / Work Gap Ledger cockpit for Code.
//!
//! Authority is external (Loop/Mapper/Runtime/Fast receipts). This module never
//! dispatches agents, approves cosign, executes effects, closes issues, or mutates
//! completion state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const COCKPIT_SCHEMA_V1: &str = "simplicio.code.agent-fabric-cockpit/v1";

/// Seven ledger phases required by the Agent Fabric work-gap model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkGapPhase {
    Unmapped,
    Owned,
    Planned,
    Implemented,
    Verified,
    Integrated,
    Delivered,
}

impl WorkGapPhase {
    pub const ALL: [Self; 7] = [
        Self::Unmapped,
        Self::Owned,
        Self::Planned,
        Self::Implemented,
        Self::Verified,
        Self::Integrated,
        Self::Delivered,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unmapped => "UNMAPPED",
            Self::Owned => "OWNED",
            Self::Planned => "PLANNED",
            Self::Implemented => "IMPLEMENTED",
            Self::Verified => "VERIFIED",
            Self::Integrated => "INTEGRATED",
            Self::Delivered => "DELIVERED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CockpitBadge {
    AddressEmitted,
    Dispatch,
    WorkerActive,
    EffectConfirmed,
    Completion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAddressView {
    pub address: String,
    pub role: String,
    #[serde(default)]
    pub active_worker: Option<String>,
    pub is_worker: bool,
    pub is_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptLink {
    pub correlation_id: String,
    pub evidence_ref: String,
    pub hbp_valid: bool,
    pub tampered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGapView {
    pub gap_id: String,
    pub phase: WorkGapPhase,
    pub ac_id: String,
    pub stage: String,
    pub agent_address: String,
    pub freshness: Freshness,
    pub blocked: bool,
    pub owner_missing: bool,
    pub orphan_contract: bool,
    pub evidence_stale: bool,
    #[serde(default)]
    pub receipt: Option<ReceiptLink>,
    #[serde(default)]
    pub badges: Vec<CockpitBadge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumView {
    pub executor: bool,
    pub verifier: bool,
    pub completion: bool,
}

impl QuorumView {
    pub fn satisfied(&self) -> bool {
        self.executor && self.verifier && self.completion
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostView {
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub cache_reuse: Option<f64>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default)]
    pub workers_avoided: Option<u32>,
    #[serde(default)]
    pub reason_if_null: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CockpitSnapshot {
    pub schema: String,
    pub repo: String,
    pub gaps: Vec<WorkGapView>,
    pub agents: Vec<AgentAddressView>,
    pub quorum: QuorumView,
    pub cost: CostView,
    pub mutable_actions_exposed: bool,
}

impl CockpitSnapshot {
    pub fn empty(repo: impl Into<String>) -> Self {
        Self {
            schema: COCKPIT_SCHEMA_V1.into(),
            repo: repo.into(),
            gaps: Vec::new(),
            agents: Vec::new(),
            quorum: QuorumView {
                executor: false,
                verifier: false,
                completion: false,
            },
            cost: CostView {
                tokens: None,
                cache_reuse: None,
                latency_ms: None,
                retries: None,
                workers_avoided: None,
                reason_if_null: Some("not_observed".into()),
            },
            mutable_actions_exposed: false,
        }
    }

    /// Fail closed: missing backend never projects green health.
    pub fn unavailable(repo: impl Into<String>, reason: impl Into<String>) -> Self {
        let mut snap = Self::empty(repo);
        snap.cost.reason_if_null = Some(reason.into());
        snap
    }

    pub fn phase_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for phase in WorkGapPhase::ALL {
            counts.insert(phase.as_str(), 0);
        }
        for gap in &self.gaps {
            *counts.entry(gap.phase.as_str()).or_insert(0) += 1;
        }
        counts
    }

    pub fn filter(
        &self,
        repo: Option<&str>,
        ac: Option<&str>,
        stage: Option<&str>,
        role: Option<&str>,
        freshness: Option<Freshness>,
    ) -> Self {
        let mut out = self.clone();
        if let Some(repo) = repo {
            if out.repo != repo {
                out.gaps.clear();
                out.agents.clear();
                return out;
            }
        }
        out.gaps.retain(|g| {
            ac.map(|v| g.ac_id == v).unwrap_or(true)
                && stage.map(|v| g.stage == v).unwrap_or(true)
                && freshness.map(|v| g.freshness == v).unwrap_or(true)
        });
        if let Some(role) = role {
            out.agents.retain(|a| a.role == role);
        }
        out
    }

    pub fn export_machine(&self) -> String {
        // TOML-like stable export for machine consumers (no write endpoints).
        let mut lines = vec![
            format!("schema = {:?}", self.schema),
            format!("repo = {:?}", self.repo),
            format!("mutable_actions_exposed = {}", self.mutable_actions_exposed),
            format!("quorum_executor = {}", self.quorum.executor),
            format!("quorum_verifier = {}", self.quorum.verifier),
            format!("quorum_completion = {}", self.quorum.completion),
            format!("gap_count = {}", self.gaps.len()),
        ];
        for (phase, count) in self.phase_counts() {
            lines.push(format!("phase_{phase} = {count}"));
        }
        lines.join("\n")
    }

    pub fn deep_link_for_gap(&self, gap_id: &str) -> Option<String> {
        self.gaps.iter().find(|g| g.gap_id == gap_id).and_then(|g| {
            g.receipt
                .as_ref()
                .map(|r| format!("simplicio://evidence/{}", r.evidence_ref))
        })
    }

    pub fn has_green_when_unknown(&self) -> bool {
        self.gaps.iter().any(|g| {
            matches!(g.freshness, Freshness::Unknown | Freshness::Stale)
                && matches!(
                    g.phase,
                    WorkGapPhase::Verified | WorkGapPhase::Integrated | WorkGapPhase::Delivered
                )
                && !g.blocked
                && !g.evidence_stale
        })
    }
}

/// Build a cockpit projection from external ledger rows. Invalid/tampered receipts block.
pub fn project_from_ledger(
    repo: &str,
    gaps: Vec<WorkGapView>,
    agents: Vec<AgentAddressView>,
    quorum: QuorumView,
    cost: CostView,
) -> CockpitSnapshot {
    let gaps = gaps
        .into_iter()
        .map(|mut g| {
            if g.receipt.as_ref().is_some_and(|r| r.tampered || !r.hbp_valid) {
                g.blocked = true;
                g.freshness = Freshness::Stale;
            }
            if g.evidence_stale {
                g.freshness = Freshness::Stale;
            }
            // Address-only never appears as worker/complete via gap badges alone.
            g
        })
        .collect();
    let agents = agents
        .into_iter()
        .map(|mut a| {
            if a.active_worker.is_none() {
                a.is_worker = false;
            }
            a
        })
        .collect();
    CockpitSnapshot {
        schema: COCKPIT_SCHEMA_V1.into(),
        repo: repo.into(),
        gaps,
        agents,
        quorum,
        cost,
        mutable_actions_exposed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_gap(phase: WorkGapPhase, fresh: Freshness) -> WorkGapView {
        WorkGapView {
            gap_id: format!("gap-{}", phase.as_str()),
            phase,
            ac_id: "AC1".into(),
            stage: "implement".into(),
            agent_address: "agent://implementer/1".into(),
            freshness: fresh,
            blocked: false,
            owner_missing: false,
            orphan_contract: false,
            evidence_stale: false,
            receipt: Some(ReceiptLink {
                correlation_id: "c1".into(),
                evidence_ref: "ev/1".into(),
                hbp_valid: true,
                tampered: false,
            }),
            badges: vec![CockpitBadge::AddressEmitted],
        }
    }

    #[test]
    fn all_seven_phases_have_representation() {
        let gaps: Vec<_> = WorkGapPhase::ALL
            .into_iter()
            .map(|p| sample_gap(p, Freshness::Fresh))
            .collect();
        let snap = project_from_ledger(
            "repo",
            gaps,
            vec![AgentAddressView {
                address: "agent://implementer/1".into(),
                role: "implementer".into(),
                active_worker: None,
                is_worker: false,
                is_complete: false,
            }],
            QuorumView {
                executor: true,
                verifier: true,
                completion: false,
            },
            CostView {
                tokens: Some(10),
                cache_reuse: Some(0.5),
                latency_ms: Some(12),
                retries: Some(0),
                workers_avoided: Some(1),
                reason_if_null: None,
            },
        );
        assert_eq!(snap.phase_counts().len(), 7);
        assert!(!snap.quorum.satisfied());
        assert!(!snap.mutable_actions_exposed);
        assert_eq!(
            snap.deep_link_for_gap("gap-OWNED").as_deref(),
            Some("simplicio://evidence/ev/1")
        );
    }

    #[test]
    fn address_only_never_marked_worker_or_complete() {
        let agents = vec![AgentAddressView {
            address: "agent://reviewer/9".into(),
            role: "reviewer".into(),
            active_worker: None,
            is_worker: true, // hostile input
            is_complete: true,
        }];
        let snap = project_from_ledger(
            "repo",
            vec![],
            agents,
            QuorumView {
                executor: false,
                verifier: false,
                completion: false,
            },
            CostView {
                tokens: None,
                cache_reuse: None,
                latency_ms: None,
                retries: None,
                workers_avoided: None,
                reason_if_null: Some("not_observed".into()),
            },
        );
        assert!(!snap.agents[0].is_worker);
    }

    #[test]
    fn tampered_receipt_blocks_gap() {
        let mut gap = sample_gap(WorkGapPhase::Verified, Freshness::Fresh);
        if let Some(r) = gap.receipt.as_mut() {
            r.tampered = true;
            r.hbp_valid = false;
        }
        let snap = project_from_ledger(
            "repo",
            vec![gap],
            vec![],
            QuorumView {
                executor: true,
                verifier: true,
                completion: true,
            },
            CostView {
                tokens: Some(1),
                cache_reuse: None,
                latency_ms: None,
                retries: None,
                workers_avoided: None,
                reason_if_null: Some("cache_not_observed".into()),
            },
        );
        assert!(snap.gaps[0].blocked);
        assert_eq!(snap.gaps[0].freshness, Freshness::Stale);
    }

    #[test]
    fn unavailable_backend_is_not_green() {
        let snap = CockpitSnapshot::unavailable("repo", "loop_hub_missing");
        assert!(snap.gaps.is_empty());
        assert!(!snap.quorum.satisfied());
        assert_eq!(snap.cost.reason_if_null.as_deref(), Some("loop_hub_missing"));
        assert!(!snap.has_green_when_unknown());
    }

    #[test]
    fn export_is_machine_readable_and_read_only() {
        let snap = CockpitSnapshot::empty("demo");
        let text = snap.export_machine();
        assert!(text.contains("mutable_actions_exposed = false"));
        assert!(text.contains("phase_UNMAPPED = 0"));
    }
}
