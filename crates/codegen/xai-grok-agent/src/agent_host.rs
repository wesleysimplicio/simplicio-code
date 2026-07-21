//! Shared Code-side adapter for one independently hosted Simplicio Agent turn.
//!
//! This crate is consumed by the interactive pager, headless entry point, ACP
//! bridge, and workspace stack. Keeping the boundary here prevents each
//! surface from growing a different AgentHost handshake or embedding an Agent
//! implementation.

use simplicio_agent_client::{
    AdvisoryPage, AgentHostCoordinator, AgentTurnCancelOutcome, AgentTurnResult, CausalIdentity,
    CoordinatorSnapshot, CoordinatorState, Error,
};
use std::sync::{Arc, Mutex, OnceLock};

static SHARED_COORDINATOR: OnceLock<Arc<Mutex<SimplicioAgentCoordinator>>> = OnceLock::new();

/// Code's versioned adapter to the one productive AgentHost coordinator.
#[derive(Debug)]
pub struct SimplicioAgentCoordinator {
    profile: String,
    inner: Option<AgentHostCoordinator>,
}

impl Default for SimplicioAgentCoordinator {
    fn default() -> Self {
        Self::new("desktop")
    }
}

impl SimplicioAgentCoordinator {
    /// Returns the process-local productive coordinator used by the TUI's
    /// asynchronous effects. The host connection and lifecycle cursor are
    /// therefore shared across turns instead of being recreated per command.
    pub fn shared() -> Arc<Mutex<Self>> {
        SHARED_COORDINATOR
            .get_or_init(|| Arc::new(Mutex::new(Self::new("desktop"))))
            .clone()
    }

    pub fn new(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            inner: None,
        }
    }

    pub fn connect(profile: impl Into<String>) -> Result<Self, Error> {
        let profile = profile.into();
        let inner = AgentHostCoordinator::connect(profile.clone())?;
        Ok(Self {
            profile,
            inner: Some(inner),
        })
    }

    fn connected(&mut self) -> Result<&mut AgentHostCoordinator, Error> {
        if self.inner.is_none() {
            self.inner = Some(AgentHostCoordinator::connect(self.profile.clone())?);
        }
        Ok(self.inner.as_mut().expect("coordinator initialized"))
    }

    /// Executes one causally identified turn against the independently
    /// running AgentHost. Connection and protocol negotiation fail closed;
    /// there is no built-in-agent or local fallback.
    pub fn start_turn(
        &mut self,
        session_id: impl Into<String>,
        message: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Result<AgentTurnResult, Error> {
        let identity = CausalIdentity::new("code", session_id, idempotency_key)?;
        self.start_turn_with_identity(&identity, message)
    }

    pub fn start_turn_with_identity(
        &mut self,
        identity: &CausalIdentity,
        message: impl Into<String>,
    ) -> Result<AgentTurnResult, Error> {
        self.connected()?.start_turn(identity, message)
    }

    pub fn cancel_turn(&mut self, turn_id: &str) -> Result<AgentTurnCancelOutcome, Error> {
        self.connected()?.cancel_turn(turn_id)
    }

    pub fn reconnect(&mut self) -> Result<CoordinatorSnapshot, Error> {
        self.connected()?.reconnect()
    }

    pub fn replay(&mut self, after: Option<u64>) -> Result<AdvisoryPage, Error> {
        self.connected()?.replay(after)
    }

    pub fn snapshot(&self) -> CoordinatorSnapshot {
        self.inner.as_ref().map_or_else(
            || CoordinatorSnapshot {
                schema: simplicio_agent_client::COORDINATOR_SNAPSHOT_SCHEMA.into(),
                profile: self.profile.clone(),
                state: CoordinatorState::Disconnected,
                cursor: 0,
                active_turn_id: None,
            },
            AgentHostCoordinator::snapshot,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_keeps_profile_as_its_identity() {
        assert_eq!(SimplicioAgentCoordinator::default().profile, "desktop");
        assert_eq!(SimplicioAgentCoordinator::new("ci").profile, "ci");
    }

    #[test]
    fn disconnected_snapshot_is_explicitly_not_ready() {
        assert_eq!(
            SimplicioAgentCoordinator::new("ci").snapshot().state,
            CoordinatorState::Disconnected
        );
    }
}
