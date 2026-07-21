//! Workspace-facing view of the shared Simplicio AgentHost coordinator.
//!
//! Workspace owns Runtime-backed tools, while AgentHost remains the sole
//! cognitive coordinator. This thin type keeps workspace callers on the same
//! Code boundary as TUI, headless, and ACP.

use simplicio_agent_client::{
    AdvisoryPage, AgentTurnCancelOutcome, AgentTurnResult, CausalIdentity, CoordinatorSnapshot,
    Error,
};
use xai_grok_agent::SimplicioAgentCoordinator;

#[derive(Debug)]
pub struct WorkspaceAgentCoordinator {
    inner: SimplicioAgentCoordinator,
}

impl WorkspaceAgentCoordinator {
    pub fn connect(profile: impl Into<String>) -> Result<Self, Error> {
        Ok(Self {
            inner: SimplicioAgentCoordinator::connect(profile)?,
        })
    }

    pub fn new(profile: impl Into<String>) -> Self {
        Self {
            inner: SimplicioAgentCoordinator::new(profile),
        }
    }

    pub fn start_turn(
        &mut self,
        identity: &CausalIdentity,
        message: impl Into<String>,
    ) -> Result<AgentTurnResult, Error> {
        self.inner.start_turn_with_identity(identity, message)
    }

    pub fn cancel_turn(&mut self, turn_id: &str) -> Result<AgentTurnCancelOutcome, Error> {
        self.inner.cancel_turn(turn_id)
    }

    pub fn reconnect(&mut self) -> Result<CoordinatorSnapshot, Error> {
        self.inner.reconnect()
    }

    pub fn replay(&mut self, after: Option<u64>) -> Result<AdvisoryPage, Error> {
        self.inner.replay(after)
    }

    pub fn snapshot(&self) -> CoordinatorSnapshot {
        self.inner.snapshot()
    }
}
