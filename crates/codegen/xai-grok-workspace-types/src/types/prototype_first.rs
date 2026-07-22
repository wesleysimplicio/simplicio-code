//! Prototype-first preview artifacts and the human decision gate.
//!
//! This module is deliberately data-only.  TUI, workspace/UI, headless, and
//! ACP callers all consume the same receipt and the same state machine.  The
//! Runtime owns persistence of the receipt/artifact bytes; this crate owns
//! validation so a client cannot accidentally turn an untrusted artifact into
//! Build authority.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const PROTOTYPE_DECISION_SCHEMA_V1: &str = "simplicio.prototype-decision/v1";
pub const PROTOTYPE_PREVIEW_SCHEMA_V1: &str = "simplicio.prototype-preview/v1";
pub const MAX_ARTIFACTS: usize = 128;
pub const MAX_PAGE_LINES: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactType {
    Wireframe,
    Diagram,
    Schema,
    DataModel,
    TestDiff,
    Benchmark,
    Storyboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl Default for RiskLevel {
    fn default() -> Self {
        Self::Low
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostEstimate {
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub amount_micros: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub label: String,
    /// Runtime-owned evidence reference. Raw project content is never part of
    /// a decision receipt or telemetry event.
    pub uri: String,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewArtifact {
    pub id: String,
    #[serde(rename = "type")]
    pub artifact_type: ArtifactType,
    pub title: String,
    pub summary: String,
    /// `runtime://prototype-first/...` or `artifact://...`; never a local
    /// file path or a path with traversal segments.
    pub uri: String,
    pub source_revision: String,
    pub digest: String,
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub ac_coverage: Vec<String>,
    #[serde(default)]
    pub risk: RiskLevel,
    #[serde(default)]
    pub cost: CostEstimate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Decision {
    Accept,
    Revise { feedback: String },
    Reject { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionAction {
    Accept,
    Revise { feedback: String },
    Reject { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopState {
    PrototypeRequired,
    CandidateGallery,
    DecisionPending,
    ReviseRequested,
    Rejected,
    BuildAuthorized,
    Stale,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionReceipt {
    pub schema: String,
    pub plan_id: String,
    pub source_revision: String,
    pub validated_source_revision: String,
    pub decision_id: String,
    pub decision: Decision,
    pub artifacts: Vec<PreviewArtifact>,
    pub assumptions: Vec<String>,
    pub limitations: Vec<String>,
    pub provenance: Vec<String>,
    pub risk: RiskLevel,
    pub cost: CostEstimate,
    pub ac_coverage: Vec<String>,
    #[serde(default)]
    pub comparison: Option<Comparison>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub left_artifact_id: String,
    pub right_artifact_id: String,
    pub changed_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub status: String,
    pub state: LoopState,
    pub build_authorized: bool,
    pub errors: Vec<String>,
    pub receipt_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrototypeLoopState {
    pub plan_id: String,
    pub source_revision: String,
    pub state: LoopState,
    #[serde(default)]
    pub receipt_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildAuthorization {
    pub schema: String,
    pub plan_id: String,
    pub decision_id: String,
    pub receipt_digest: String,
    pub source_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Tui,
    Ui,
    Headless,
    Acp,
}

impl Surface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Ui => "ui",
            Self::Headless => "headless",
            Self::Acp => "acp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryDecision {
    pub event: &'static str,
    pub receipt_digest: String,
    pub plan_id_digest: String,
    pub artifact_ids: Vec<String>,
    pub decision: String,
    pub state: LoopState,
    pub risk: RiskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessibilityAudit {
    pub keyboard_actions: Vec<&'static str>,
    pub has_text_labels: bool,
    pub has_risk_and_limitations: bool,
    pub contrast_ratio_x100: u16,
}

impl AccessibilityAudit {
    pub fn passes(&self) -> bool {
        !self.keyboard_actions.is_empty()
            && self.has_text_labels
            && self.has_risk_and_limitations
            && self.contrast_ratio_x100 >= 450
    }
}

impl DecisionReceipt {
    pub fn with_decision(&self, action: DecisionAction) -> Self {
        let mut next = self.clone();
        next.decision = match action {
            DecisionAction::Accept => Decision::Accept,
            DecisionAction::Revise { feedback } => Decision::Revise { feedback },
            DecisionAction::Reject { reason } => Decision::Reject { reason },
        };
        next.validated_source_revision = next.source_revision.clone();
        next
    }

    pub fn validate(
        &self,
        current_source_revision: &str,
        build_requested: bool,
    ) -> ValidationReport {
        let mut errors = Vec::new();
        if self.schema != PROTOTYPE_DECISION_SCHEMA_V1 {
            errors.push(format!("schema must be {PROTOTYPE_DECISION_SCHEMA_V1}"));
        }
        for (name, value) in [
            ("plan_id", self.plan_id.as_str()),
            ("source_revision", self.source_revision.as_str()),
            (
                "validated_source_revision",
                self.validated_source_revision.as_str(),
            ),
            ("decision_id", self.decision_id.as_str()),
        ] {
            if value.trim().is_empty() || !safe_text(value) {
                errors.push(format!("{name} is required and must be safe text"));
            }
        }
        if self.source_revision != self.validated_source_revision
            || self.source_revision != current_source_revision
        {
            errors.push("source drift invalidates the prototype decision".into());
        }
        if self.artifacts.is_empty() {
            errors.push("at least one prototype artifact is required".into());
        }
        if self.artifacts.len() > MAX_ARTIFACTS {
            errors.push(format!("too many artifacts (maximum {MAX_ARTIFACTS})"));
        }
        let mut ids = BTreeSet::new();
        for (index, artifact) in self.artifacts.iter().enumerate() {
            if !ids.insert(&artifact.id) {
                errors.push(format!("duplicate artifact id {}", artifact.id));
            }
            if !safe_id(&artifact.id) {
                errors.push(format!("artifacts[{index}] has an unsafe id"));
            }
            for (name, value) in [
                ("title", artifact.title.as_str()),
                ("summary", artifact.summary.as_str()),
            ] {
                if value.trim().is_empty() || !safe_text(value) {
                    errors.push(format!("artifacts[{index}] {name} is empty or unsafe"));
                }
            }
            if !safe_uri(&artifact.uri) {
                errors.push(format!(
                    "artifacts[{index}] uri escapes the artifact sandbox"
                ));
            }
            if artifact.source_revision != self.source_revision {
                errors.push(format!(
                    "artifacts[{index}] source revision differs from receipt"
                ));
            }
            if artifact.digest.trim().is_empty() || !safe_digest(&artifact.digest) {
                errors.push(format!("artifacts[{index}] digest is required"));
            }
            if artifact.evidence.is_empty() {
                errors.push(format!("artifacts[{index}] requires evidence"));
            }
            for evidence in &artifact.evidence {
                if !safe_id(&evidence.id) || !safe_text(&evidence.label) || !safe_uri(&evidence.uri)
                {
                    errors.push(format!("artifact {} contains unsafe evidence", artifact.id));
                }
            }
            if artifact.ac_coverage.is_empty() {
                errors.push(format!("artifacts[{index}] requires AC coverage"));
            }
        }
        if let Some(comparison) = &self.comparison {
            match self.compare(&comparison.left_artifact_id, &comparison.right_artifact_id) {
                Ok(expected) if expected == *comparison => {}
                Ok(_) => errors
                    .push("comparison changed_fields does not match the selected artifacts".into()),
                Err(error) => errors.push(error),
            }
        }
        if self.assumptions.iter().any(|v| !safe_text(v))
            || self.limitations.iter().any(|v| !safe_text(v))
            || self.provenance.iter().any(|v| !safe_uri(v))
        {
            errors.push("assumptions, limitations, or provenance contains unsafe text".into());
        }
        if self.ac_coverage.is_empty() {
            errors.push("acceptance-criteria coverage is required".into());
        }
        if matches!(&self.decision, Decision::Revise { feedback } if feedback.trim().is_empty()) {
            errors.push("revise requires feedback".into());
        }
        if matches!(&self.decision, Decision::Reject { reason } if reason.trim().is_empty()) {
            errors.push("reject requires a reason".into());
        }
        let digest = receipt_digest(self);
        let stale = errors.iter().any(|error| error.contains("source drift"));
        let state = if stale {
            LoopState::Stale
        } else if errors.is_empty() && matches!(self.decision, Decision::Accept) {
            LoopState::BuildAuthorized
        } else if matches!(self.decision, Decision::Revise { .. }) {
            LoopState::ReviseRequested
        } else if matches!(self.decision, Decision::Reject { .. }) {
            LoopState::Rejected
        } else if errors.is_empty() {
            LoopState::DecisionPending
        } else {
            LoopState::Blocked
        };
        let build_authorized =
            build_requested && errors.is_empty() && matches!(self.decision, Decision::Accept);
        if build_requested && !build_authorized {
            errors.push("Build requires a valid, current ACCEPT decision".into());
        }
        ValidationReport {
            status: if errors.is_empty() {
                "ready"
            } else {
                "blocked"
            }
            .into(),
            state,
            build_authorized,
            errors,
            receipt_digest: digest,
        }
    }

    pub fn authorize_build(
        &self,
        current_source_revision: &str,
    ) -> Result<BuildAuthorization, ValidationReport> {
        let report = self.validate(current_source_revision, true);
        if report.build_authorized {
            Ok(BuildAuthorization {
                schema: "simplicio.build-authorization/v1".into(),
                plan_id: self.plan_id.clone(),
                decision_id: self.decision_id.clone(),
                receipt_digest: report.receipt_digest,
                source_revision: current_source_revision.into(),
            })
        } else {
            Err(report)
        }
    }

    pub fn compare(&self, left: &str, right: &str) -> Result<Comparison, String> {
        if !safe_id(left) || !safe_id(right) {
            return Err("comparison contains an unsafe artifact id".into());
        }
        if left == right {
            return Err("comparison requires two distinct artifacts".into());
        }
        let left_artifact = self.artifacts.iter().find(|item| item.id == left);
        let right_artifact = self.artifacts.iter().find(|item| item.id == right);
        let (Some(left_artifact), Some(right_artifact)) = (left_artifact, right_artifact) else {
            return Err("comparison references an unknown artifact".into());
        };
        let mut changed = Vec::new();
        if left_artifact.artifact_type != right_artifact.artifact_type {
            changed.push("type".into());
        }
        if left_artifact.summary != right_artifact.summary {
            changed.push("summary".into());
        }
        if left_artifact.digest != right_artifact.digest {
            changed.push("digest".into());
        }
        if left_artifact.ac_coverage != right_artifact.ac_coverage {
            changed.push("ac_coverage".into());
        }
        if left_artifact.risk != right_artifact.risk {
            changed.push("risk".into());
        }
        Ok(Comparison {
            left_artifact_id: left.into(),
            right_artifact_id: right.into(),
            changed_fields: changed,
        })
    }

    pub fn telemetry(&self, current_source_revision: &str) -> TelemetryDecision {
        let report = self.validate(current_source_revision, false);
        let decision = match &self.decision {
            Decision::Accept => "accept",
            Decision::Revise { .. } => "revise",
            Decision::Reject { .. } => "reject",
        };
        TelemetryDecision {
            event: "prototype_decision",
            receipt_digest: report.receipt_digest,
            plan_id_digest: digest_text(&self.plan_id),
            artifact_ids: self
                .artifacts
                .iter()
                .map(|artifact| artifact.id.clone())
                .collect(),
            decision: decision.into(),
            state: report.state,
            risk: self.risk,
        }
    }
}

impl PrototypeLoopState {
    pub fn new(plan_id: impl Into<String>, source_revision: impl Into<String>) -> Self {
        Self {
            plan_id: plan_id.into(),
            source_revision: source_revision.into(),
            state: LoopState::PrototypeRequired,
            receipt_digest: None,
        }
    }

    pub fn publish(&mut self, receipt: &DecisionReceipt) -> ValidationReport {
        let mut report = receipt.validate(&self.source_revision, false);
        if receipt.plan_id != self.plan_id {
            report
                .errors
                .push("receipt plan does not match Loop state".into());
            report.status = "blocked".into();
            report.state = LoopState::Blocked;
        }
        self.state = if report.errors.is_empty() {
            LoopState::DecisionPending
        } else {
            report.state
        };
        self.receipt_digest = Some(report.receipt_digest.clone());
        report
    }

    pub fn source_changed(&mut self, source_revision: impl Into<String>) {
        self.source_revision = source_revision.into();
        self.state = LoopState::Stale;
        self.receipt_digest = None;
    }

    pub fn authorize_build(
        &mut self,
        receipt: &DecisionReceipt,
    ) -> Result<BuildAuthorization, ValidationReport> {
        let authorization = receipt.authorize_build(&self.source_revision);
        match &authorization {
            Ok(value) => {
                self.state = LoopState::BuildAuthorized;
                self.receipt_digest = Some(value.receipt_digest.clone());
            }
            Err(report) => self.state = report.state,
        }
        authorization
    }
}

pub fn render_surface(
    receipt: &DecisionReceipt,
    current_source_revision: &str,
    surface: Surface,
) -> Result<String, ValidationReport> {
    let report = receipt.validate(current_source_revision, false);
    if surface == Surface::Tui {
        return Ok(render_tui(receipt, &report));
    }
    let payload = serde_json::json!({
        "schema": PROTOTYPE_PREVIEW_SCHEMA_V1,
        "surface": surface.as_str(),
        "state": report.state,
        "status": report.status,
        "plan_id": receipt.plan_id,
        "decision": receipt.decision,
        "artifacts": receipt.artifacts,
        "assumptions": receipt.assumptions,
        "limitations": receipt.limitations,
        "provenance": receipt.provenance,
        "risk": receipt.risk,
        "cost": receipt.cost,
        "ac_coverage": receipt.ac_coverage,
        "actions": ["compare", "accept", "revise", "reject"],
        "build_authorized": report.build_authorized,
        "errors": report.errors,
    });
    serde_json::to_string_pretty(&payload).map_err(|error| ValidationReport {
        status: "error".into(),
        state: LoopState::Blocked,
        build_authorized: false,
        errors: vec![error.to_string()],
        receipt_digest: report.receipt_digest,
    })
}

pub fn render_tui(receipt: &DecisionReceipt, report: &ValidationReport) -> String {
    let mut lines = vec![
        "PROTOTYPE PREVIEW".to_string(),
        format!(
            "Plan: {} | State: {:?} | Build: {}",
            safe_display(&receipt.plan_id),
            report.state,
            if report.build_authorized {
                "AUTHORIZED"
            } else {
                "BLOCKED"
            }
        ),
        format!("Decision: {}", decision_name(&receipt.decision)),
        "Candidates:".into(),
    ];
    for artifact in receipt
        .artifacts
        .iter()
        .take(MAX_PAGE_LINES.saturating_sub(8))
    {
        lines.push(format!(
            "  [{}] {:?}: {} — {}",
            safe_display(&artifact.id),
            artifact.artifact_type,
            safe_display(&artifact.title),
            safe_display(&artifact.summary)
        ));
        lines.push(format!(
            "      evidence: {} | AC: {} | risk: {:?}",
            artifact.evidence.len(),
            artifact.ac_coverage.len(),
            artifact.risk
        ));
    }
    lines.push(format!(
        "Assumptions: {} | Limitations: {} | Cost: {} tokens",
        receipt.assumptions.len(),
        receipt.limitations.len(),
        receipt.cost.tokens
    ));
    lines.push("Actions: [compare] [accept] [revise] [reject] [page]".into());
    if !report.errors.is_empty() {
        lines.push(format!("Blocked: {}", report.errors.join("; ")));
    }
    lines.join("\n")
}

pub fn paginate(text: &str, page: usize, lines_per_page: usize) -> String {
    let lines_per_page = lines_per_page.clamp(1, MAX_PAGE_LINES);
    text.lines()
        .skip(page.saturating_mul(lines_per_page))
        .take(lines_per_page)
        .map(safe_display)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn accessibility_audit() -> AccessibilityAudit {
    AccessibilityAudit {
        keyboard_actions: vec!["compare", "accept", "revise", "reject", "page"],
        has_text_labels: true,
        has_risk_and_limitations: true,
        contrast_ratio_x100: 450,
    }
}

fn safe_text(value: &str) -> bool {
    !value
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}
fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
fn safe_digest(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}
fn safe_uri(value: &str) -> bool {
    if !safe_text(value)
        || value.is_empty()
        || value.starts_with("file:")
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || !(value.starts_with("artifact://")
            || value.starts_with("runtime://")
            || !value.contains("://"))
    {
        return false;
    }

    // Decode percent escapes before checking path segments. Otherwise values
    // such as `artifact://%2e%2e/secret` pass the literal `..` check and can
    // escape when a downstream URI implementation normalizes them.
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return false;
            }
            let Some(high) = hex_value(bytes[index + 1]) else {
                return false;
            };
            let Some(low) = hex_value(bytes[index + 2]) else {
                return false;
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    if decoded.contains(&b'\\') || decoded.contains(&0) {
        return false;
    }
    !decoded
        .split(|byte| *byte == b'/')
        .any(|part| part == b"..")
}
fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
fn safe_display(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(512)
        .collect()
}
fn decision_name(value: &Decision) -> &'static str {
    match value {
        Decision::Accept => "accept",
        Decision::Revise { .. } => "revise",
        Decision::Reject { .. } => "reject",
    }
}
fn digest_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}
fn receipt_digest(receipt: &DecisionReceipt) -> String {
    serde_json::to_vec(receipt)
        .map(|bytes| {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            format!("sha256:{:x}", hasher.finalize())
        })
        .unwrap_or_else(|_| "sha256:invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(id: &str) -> PreviewArtifact {
        PreviewArtifact {
            id: id.into(),
            artifact_type: ArtifactType::Wireframe,
            title: "Home".into(),
            summary: "Main flow".into(),
            uri: format!("runtime://prototype-first/{id}"),
            source_revision: "source-1".into(),
            digest: "sha256:artifact".into(),
            evidence: vec![Evidence {
                id: "e1".into(),
                label: "Acceptance test".into(),
                uri: "runtime://evidence/e1".into(),
                digest: None,
            }],
            assumptions: vec!["existing API".into()],
            limitations: vec!["preview only".into()],
            ac_coverage: vec!["AC-1".into()],
            risk: RiskLevel::Low,
            cost: CostEstimate {
                tokens: 12,
                ..Default::default()
            },
        }
    }

    fn receipt(decision: Decision) -> DecisionReceipt {
        DecisionReceipt {
            schema: PROTOTYPE_DECISION_SCHEMA_V1.into(),
            plan_id: "plan-1".into(),
            source_revision: "source-1".into(),
            validated_source_revision: "source-1".into(),
            decision_id: "decision-1".into(),
            decision,
            artifacts: vec![artifact("a1"), artifact("a2")],
            assumptions: vec!["uses existing API".into()],
            limitations: vec!["not production code".into()],
            provenance: vec!["runtime://map/repo".into()],
            risk: RiskLevel::Low,
            cost: CostEstimate::default(),
            ac_coverage: vec!["AC-1".into()],
            comparison: None,
        }
    }

    #[test]
    fn current_accept_authorizes_build() {
        assert!(
            receipt(Decision::Accept)
                .authorize_build("source-1")
                .is_ok()
        );
    }

    #[test]
    fn revise_reject_and_stale_block_build() {
        assert!(
            receipt(Decision::Revise {
                feedback: "change layout".into()
            })
            .authorize_build("source-1")
            .is_err()
        );
        assert!(
            receipt(Decision::Reject {
                reason: "wrong flow".into()
            })
            .authorize_build("source-1")
            .is_err()
        );
        let report = receipt(Decision::Accept).validate("source-2", true);
        assert_eq!(report.state, LoopState::Stale);
        assert!(!report.build_authorized);
    }

    #[test]
    fn compare_and_render_are_surface_consistent() {
        let value = receipt(Decision::Accept);
        let comparison = value.compare("a1", "a2").unwrap();
        assert!(comparison.changed_fields.is_empty());
        for surface in [Surface::Tui, Surface::Ui, Surface::Headless, Surface::Acp] {
            let rendered = render_surface(&value, "source-1", surface).unwrap();
            assert!(rendered.contains("accept") || rendered.contains("Accept"));
        }
    }

    #[test]
    fn malicious_artifact_and_controls_are_blocked() {
        let mut value = receipt(Decision::Accept);
        value.artifacts[0].uri = "file:///etc/passwd".into();
        value.artifacts[0].title = "\u{1b}[31mowned".into();
        let report = value.validate("source-1", false);
        assert_eq!(report.status, "blocked");
        assert!(report.errors.iter().any(|error| error.contains("sandbox")));
    }

    #[test]
    fn encoded_and_windows_artifact_traversal_are_blocked() {
        for uri in [
            "artifact://%2e%2e/secret",
            "runtime://prototype-first/%2E%2E/secret",
            "artifact://candidate\\..\\secret",
            "artifact://candidate/%00secret",
            "artifact://candidate/%zz",
        ] {
            let mut value = receipt(Decision::Accept);
            value.artifacts[0].uri = uri.into();
            assert!(
                value
                    .validate("source-1", false)
                    .errors
                    .iter()
                    .any(|error| error.contains("sandbox"))
            );
        }
    }

    #[test]
    fn comparison_receipt_is_recomputed_and_fail_closed() {
        let mut value = receipt(Decision::Accept);
        value.artifacts[1].summary = "Alternate flow".into();
        value.comparison = Some(value.compare("a1", "a2").unwrap());
        assert!(value.validate("source-1", false).errors.is_empty());

        value.comparison.as_mut().unwrap().changed_fields.clear();
        assert!(
            value
                .validate("source-1", false)
                .errors
                .iter()
                .any(|error| error.contains("changed_fields"))
        );
        assert!(value.compare("a1", "a1").is_err());
        assert!(value.compare("../a1", "a2").is_err());
    }

    #[test]
    fn pagination_and_accessibility_are_bounded() {
        let text = (0..500)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(paginate(&text, 4, 50).lines().count(), 50);
        assert!(accessibility_audit().passes());
    }

    #[test]
    fn telemetry_redacts_plan_content() {
        let mut value = receipt(Decision::Revise {
            feedback: "secret prompt and code".into(),
        });
        value.plan_id = "plan with secret".into();
        let telemetry = value.telemetry("source-1");
        let json = serde_json::to_string(&telemetry).unwrap();
        assert!(!json.contains("secret prompt"));
        assert!(!json.contains("plan with secret"));
    }

    #[test]
    fn loop_state_tracks_publish_drift_and_build_gate() {
        let accepted = receipt(Decision::Accept);
        let mut loop_state = PrototypeLoopState::new("plan-1", "source-1");
        assert_eq!(
            loop_state.publish(&accepted).state,
            LoopState::DecisionPending
        );
        assert!(loop_state.authorize_build(&accepted).is_ok());
        loop_state.source_changed("source-2");
        assert_eq!(loop_state.state, LoopState::Stale);
        assert!(loop_state.authorize_build(&accepted).is_err());
    }
}
