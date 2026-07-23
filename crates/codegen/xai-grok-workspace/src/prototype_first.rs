//! Workspace/UI adapter for Runtime-owned Prototype-First artifacts.

use xai_grok_workspace_types::{DecisionReceipt, PrototypePanel, Surface, ValidationReport};

/// Render the workspace surface from the shared receipt contract. Artifact
/// bytes are fetched by the Runtime; this adapter never reads them directly.
pub fn render_workspace_preview(
    receipt: &DecisionReceipt,
    source_revision: &str,
) -> Result<String, ValidationReport> {
    xai_grok_workspace_types::render_surface(receipt, source_revision, Surface::Ui)
}

/// Render the same interactive gallery state used by the terminal and ACP
/// adapters, tagged as the workspace surface.
pub fn render_workspace_panel(panel: &PrototypePanel) -> Result<String, serde_json::Error> {
    panel.render(Surface::Ui)
}
