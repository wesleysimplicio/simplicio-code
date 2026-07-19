# CoordinatorProtocol/v1

`scripts/validate_coordinator_protocol.py` defines the neutral contract for
the optional coordinator requested by issue #50. A Code turn names exactly one
coordinator (`builtin`, `simplicio-agent`, or `external`) and carries stable
workspace/session/turn/policy identities. Events must have unique increasing
sequence numbers and causal IDs; invalid lifecycle transitions are blocked.

This contract is intentionally independent of the Agent implementation. The
future AgentHost adapter must emit this envelope and keep Runtime effects out
of the coordinator transport. The validator is a boundary gate, not evidence
that an AgentHost is already shipped in this repository.
