//! Non-focusable side panel for the Simplicio Agent host projection.

use crate::app::agent_attention::{
    AgentAttentionPanelState, AgentAttentionResyncState, AgentHostStatus,
};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap};
use simplicio_agent_client::AdvisorySeverity;

const PANEL_WIDTH: u16 = 32;
const PANEL_GAP: u16 = 1;
const MIN_AGENT_WIDTH: u16 = 76;
const MIN_PANEL_HEIGHT: u16 = 8;

/// Reserve a passive right rail only when the main agent view remains roomy.
/// Narrow terminals retain the existing full-width interaction surface.
pub fn split_agent_area(area: Rect) -> (Rect, Option<Rect>) {
    let required = MIN_AGENT_WIDTH + PANEL_GAP + PANEL_WIDTH;
    if area.width < required || area.height < MIN_PANEL_HEIGHT {
        return (area, None);
    }
    let agent_width = area.width - PANEL_GAP - PANEL_WIDTH;
    let panel = Rect {
        x: area.x + agent_width + PANEL_GAP,
        y: area.y,
        width: PANEL_WIDTH,
        height: area.height,
    };
    (
        Rect {
            width: agent_width,
            ..area
        },
        Some(panel),
    )
}

/// Render status and generic host advisories. The widget exposes no hit rect,
/// cursor, keybinding, or action callback, so it cannot take focus or execute
/// the Agent's suggested action.
pub fn render_agent_attention_panel(
    area: Rect,
    buf: &mut Buffer,
    state: &AgentAttentionPanelState,
) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    let theme = Theme::current();
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.gray_dim))
        .title(Span::styled(
            " Simplicio Agent ",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = Vec::<Line<'static>>::new();
    match &state.status {
        AgentHostStatus::Connecting => lines.push(Line::from(Span::styled(
            "○ connecting",
            Style::default().fg(theme.gray),
        ))),
        AgentHostStatus::Ready { profile } => {
            lines.push(Line::from(Span::styled(
                "● ready",
                Style::default()
                    .fg(theme.accent_success)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!("profile: {}", profile.label()),
                Style::default().fg(theme.gray),
            )));
        }
        AgentHostStatus::Degraded { reason } => {
            lines.push(Line::from(Span::styled(
                "! DEGRADED",
                Style::default()
                    .fg(theme.accent_error)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                reason.message(),
                Style::default().fg(theme.text_secondary),
            )));
            lines.push(Line::from(Span::styled(
                format!("reason: {}", reason.code()),
                Style::default().fg(theme.gray),
            )));
            if state.attention().is_some() {
                lines.push(Line::from(Span::styled(
                    "showing last known data",
                    Style::default().fg(theme.gray),
                )));
            }
        }
    }

    if state.resync == AgentAttentionResyncState::RestartResync {
        lines.push(Line::from(Span::styled(
            "↻ restart_resync",
            Style::default().fg(theme.accent_system),
        )));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Host advisories",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(attention) = state.attention() {
        lines.push(Line::from(Span::styled(
            format!("{} captured", attention.unread),
            Style::default().fg(theme.gray_bright),
        )));
        if let Some(severity) = attention.highest_severity {
            let (label, color) = match severity {
                AdvisorySeverity::Info => ("info", theme.accent_system),
                AdvisorySeverity::Warning => ("warning", theme.warning),
            };
            lines.push(Line::from(vec![
                Span::styled("severity: ", Style::default().fg(theme.gray)),
                Span::styled(label, Style::default().fg(color)),
            ]));
        }
        if let Some(summary) = &attention.latest_summary {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "Latest",
                Style::default().fg(theme.gray),
            )));
            lines.push(Line::from(Span::styled(
                summary.clone(),
                Style::default().fg(theme.text_primary),
            )));
        }
        if let Some(action) = &attention.suggested_action {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "Suggested (not run)",
                Style::default().fg(theme.gray),
            )));
            lines.push(Line::from(Span::styled(
                action.clone(),
                Style::default().fg(theme.warning),
            )));
        }
        if attention.history_truncated {
            lines.push(Line::from(Span::styled(
                "history truncated",
                Style::default().fg(theme.warning),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No advisories yet",
            Style::default().fg(theme.gray),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Passive: no actions run",
        Style::default().fg(theme.gray_dim),
    )));

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .render(inner, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_attention::AgentAttentionPollResult;
    use simplicio_agent_client::{AgentAttentionState, HostInstanceId};

    const HOST_A: &str = "panel-host-instance-aaaa";
    const HOST_B: &str = "panel-host-instance-bbbb";

    fn complete_ready(
        state: &mut AgentAttentionPanelState,
        host: &str,
        replayed_from_cursor: u64,
        profile: crate::app::agent_attention::AgentHostProfile,
        attention: AgentAttentionState,
    ) {
        let request = state.begin_poll().unwrap();
        state.complete_poll(AgentAttentionPollResult::Ready {
            request,
            host_instance_id: HostInstanceId::from_untrusted(host).unwrap(),
            replayed_from_cursor,
            profile,
            attention,
        });
    }

    fn buffer_text(buf: &Buffer) -> String {
        let area = buf.area;
        let mut out = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn wide_layout_reserves_panel_without_shrinking_below_main_minimum() {
        let area = Rect::new(4, 2, 120, 30);
        let (agent, panel) = split_agent_area(area);
        let panel = panel.unwrap();
        assert_eq!(agent, Rect::new(4, 2, 87, 30));
        assert_eq!(panel, Rect::new(92, 2, 32, 30));
    }

    #[test]
    fn narrow_layout_preserves_the_existing_agent_area() {
        let area = Rect::new(0, 0, 108, 30);
        assert_eq!(split_agent_area(area), (area, None));
    }

    #[test]
    fn degraded_state_is_explicit_and_passive() {
        let mut state = AgentAttentionPanelState::default();
        let request = state.begin_poll().unwrap();
        state.complete_poll(AgentAttentionPollResult::Degraded {
            request,
            reason: crate::app::agent_attention::AgentHostDegradedReason::AgentUnavailable,
        });
        let area = Rect::new(0, 0, 32, 12);
        let mut buf = Buffer::empty(area);
        render_agent_attention_panel(area, &mut buf, &state);
        let text = buffer_text(&buf);
        assert!(text.contains("DEGRADED"));
        assert!(text.contains("Agent host is unavailable."));
        assert!(text.contains("reason: agent_unavailable"));
        assert!(text.contains("Passive: no actions run"));
    }

    #[test]
    fn advisory_action_is_labeled_as_not_run() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            0,
            crate::app::agent_attention::AgentHostProfile::Desktop,
            AgentAttentionState {
                cursor: 7,
                unread: 1,
                highest_severity: Some(AdvisorySeverity::Warning),
                latest_summary: Some("Agent host is saturated.".into()),
                suggested_action: Some("retry".into()),
                history_truncated: true,
            },
        );
        let area = Rect::new(0, 0, 32, 18);
        let mut buf = Buffer::empty(area);
        render_agent_attention_panel(area, &mut buf, &state);
        let text = buffer_text(&buf);
        assert!(text.contains("Agent host is saturated."));
        assert!(text.contains("Suggested (not run)"));
        assert!(text.contains("retry"));
    }

    #[test]
    fn hostile_profile_and_local_path_never_reach_the_buffer() {
        let raw_error = simplicio_agent_client::Error::AgentNotFound(std::path::PathBuf::from(
            "/home/user/private/secret.sock",
        ));
        let safe_reason = crate::app::agent_attention::safe_reason(&raw_error);
        let mut degraded = AgentAttentionPanelState::default();
        let request = degraded.begin_poll().unwrap();
        degraded.complete_poll(AgentAttentionPollResult::Degraded {
            request,
            reason: safe_reason,
        });
        let area = Rect::new(0, 0, 32, 12);
        let mut degraded_buf = Buffer::empty(area);
        render_agent_attention_panel(area, &mut degraded_buf, &degraded);
        let degraded_text = buffer_text(&degraded_buf);
        assert!(!degraded_text.contains("/home/user/private/secret.sock"));

        let hostile_profile = format!("secret\n\u{1b}[31m{}", "x".repeat(10_000));
        let profile =
            crate::app::agent_attention::AgentHostProfile::from_untrusted(&hostile_profile);
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            0,
            profile,
            AgentAttentionState {
                cursor: 0,
                unread: 0,
                highest_severity: None,
                latest_summary: None,
                suggested_action: None,
                history_truncated: false,
            },
        );
        let mut buf = Buffer::empty(area);
        render_agent_attention_panel(area, &mut buf, &state);
        let text = buffer_text(&buf);
        assert!(text.contains("profile: compatible"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("[31m"));
        assert!(!text.contains("/home/user/private"));
    }

    #[test]
    fn committed_restart_shows_only_the_fixed_resync_marker() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            0,
            crate::app::agent_attention::AgentHostProfile::Desktop,
            AgentAttentionState {
                cursor: 1,
                unread: 1,
                highest_severity: Some(AdvisorySeverity::Info),
                latest_summary: Some("Agent host is ready.".into()),
                suggested_action: None,
                history_truncated: false,
            },
        );
        complete_ready(
            &mut state,
            HOST_B,
            0,
            crate::app::agent_attention::AgentHostProfile::Desktop,
            AgentAttentionState {
                cursor: 1,
                unread: 1,
                highest_severity: Some(AdvisorySeverity::Info),
                latest_summary: Some("Agent host is ready.".into()),
                suggested_action: None,
                history_truncated: false,
            },
        );

        let area = Rect::new(0, 0, 32, 18);
        let mut buf = Buffer::empty(area);
        render_agent_attention_panel(area, &mut buf, &state);
        let text = buffer_text(&buf);
        assert!(text.contains("restart_resync"));
        assert!(!text.contains(HOST_A));
        assert!(!text.contains(HOST_B));
    }
}
