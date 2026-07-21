//! Shared Code-side adapter for one independently hosted Simplicio Agent turn.
//!
//! This crate is consumed by both the interactive pager and the shell/workspace
//! stack. Keeping the boundary here prevents each surface from growing a
//! different AgentHost handshake or from embedding an Agent implementation.

use simplicio_agent_client::{AgentHostClient, AgentTurnRequest, AgentTurnResult, Error};

/// Code's versioned adapter for a productive AgentHost turn.
///
/// The Runtime remains the effect boundary. This adapter only establishes the
/// cognitive-turn contract; callers must still route any tool effect through
/// the Runtime-backed tool configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimplicioAgentCoordinator {
    profile: String,
}

impl Default for SimplicioAgentCoordinator {
    fn default() -> Self {
        Self::new("desktop")
    }
}

impl SimplicioAgentCoordinator {
    pub fn new(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
        }
    }

    /// Executes one causally identified turn against the independently running
    /// AgentHost. Connection and protocol negotiation fail closed; there is no
    /// built-in-agent or local fallback.
    pub fn start_turn(
        &self,
        session_id: impl Into<String>,
        message: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Result<AgentTurnResult, Error> {
        let request = AgentTurnRequest::new(&self.profile, session_id, message, idempotency_key)?;
        AgentHostClient::connect_default()?.start_turn(&request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_keeps_profile_as_part_of_its_identity() {
        assert_eq!(SimplicioAgentCoordinator::default().profile, "desktop");
        assert_eq!(SimplicioAgentCoordinator::new("ci").profile, "ci");
    }
}
