//! Pager adapters for the shared Prototype-First contract.
//!
//! Rendering stays in `xai-grok-workspace-types`; this module only names the
//! surface so TUI, headless, and ACP entry points cannot drift semantically.

use xai_grok_workspace_types::PrototypePanel;
pub use xai_grok_workspace_types::prototype_first::*;
use xai_grok_workspace_types::{DecisionReceipt, Surface, ValidationReport};

pub fn render_tui_preview(receipt: &DecisionReceipt, source_revision: &str) -> String {
    let report = receipt.validate(source_revision, false);
    xai_grok_workspace_types::render_tui(receipt, &report)
}
pub fn render_ui_preview(
    receipt: &DecisionReceipt,
    source_revision: &str,
) -> Result<String, ValidationReport> {
    xai_grok_workspace_types::render_surface(receipt, source_revision, Surface::Ui)
}

pub fn render_acp_preview(
    receipt: &DecisionReceipt,
    source_revision: &str,
) -> Result<String, ValidationReport> {
    xai_grok_workspace_types::render_surface(receipt, source_revision, Surface::Acp)
}

/// Render the interactive gallery's canonical semantic model for terminal,
/// headless, or ACP consumers.
pub fn render_panel(panel: &PrototypePanel, surface: Surface) -> Result<String, serde_json::Error> {
    panel.render(surface)
}
