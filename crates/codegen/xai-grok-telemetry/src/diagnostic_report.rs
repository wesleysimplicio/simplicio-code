//! `simplicio-code privacy diagnose` — a pure, side-effect-free report of
//! what telemetry *would* be sent, and where, given the current effective
//! configuration.
//!
//! Built for issue "privacy: telemetria mínima e controles de dados":
//! acceptance criteria include "usuário consegue listar o que seria
//! enviado" (the user can list what would be sent) and "opt-out é
//! respeitado antes do primeiro evento". This module never makes a network
//! call and never emits a telemetry event itself — it only inspects config
//! and returns a structured, printable summary. Callers (CLI, tests, other
//! front-ends) decide how to render [`DiagnosticReport`].

use serde::Serialize;

use crate::config::{TelemetryConfig, TelemetryMode, do_not_track_requested};

/// One outbound destination the client may contact for telemetry/crash
/// reporting/tracing purposes, and whether it is currently active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticDestination {
    /// Stable machine-readable id (e.g. `"mixpanel"`).
    pub id: &'static str,
    /// Host(s) contacted, for display (no path/query — those never leave
    /// the allowlisted shape documented in `docs/privacy/telemetry.md`).
    pub host: String,
    /// One-line purpose, in the allowlist-schema sense (issue #13 step 3:
    /// "schema allowlist com finalidade e retenção").
    pub purpose: &'static str,
    /// Whether this destination would currently be contacted given the
    /// resolved config passed to [`build_diagnostic_report`].
    pub active: bool,
    /// Why `active` has this value (env var / config key / default), for a
    /// user who wants to know *why* something is on or off.
    pub reason: String,
}

/// Full diagnostic snapshot: overall telemetry mode plus one row per known
/// destination. `active_destinations()` gives the subset actually reachable
/// right now — that list, rendered as text, is exactly what `/privacy
/// diagnose` or `simplicio-code privacy diagnose` prints to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticReport {
    /// Effective `TelemetryMode` (`disabled` / `session_metrics` / `true`).
    pub telemetry_mode: String,
    /// `true` when `DO_NOT_TRACK` (or an equivalent opt-out) is in effect,
    /// regardless of `telemetry_mode` — see [`do_not_track_requested`].
    pub opted_out_via_do_not_track: bool,
    pub destinations: Vec<DiagnosticDestination>,
    /// Fields the allowlist schema explicitly forbids from ever being
    /// attached to an event (issue #13 step 4). Documented here so the
    /// diagnose output doubles as a live contract check.
    pub never_sent_fields: Vec<&'static str>,
}

impl DiagnosticReport {
    pub fn active_destinations(&self) -> impl Iterator<Item = &DiagnosticDestination> {
        self.destinations.iter().filter(|d| d.active)
    }

    /// Render as human-readable lines, in the order a CLI would print them.
    pub fn to_text_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("Telemetry mode: {}", self.telemetry_mode));
        if self.opted_out_via_do_not_track {
            lines.push("DO_NOT_TRACK: honored — all telemetry disabled.".to_owned());
        }
        lines.push(String::new());
        lines.push("Destinations:".to_owned());
        for dest in &self.destinations {
            let status = if dest.active { "ACTIVE" } else { "inactive" };
            lines.push(format!(
                "  [{status}] {} ({}) — {} [{}]",
                dest.id, dest.host, dest.purpose, dest.reason
            ));
        }
        lines.push(String::new());
        lines.push("Fields never sent to any telemetry destination:".to_owned());
        for field in &self.never_sent_fields {
            lines.push(format!("  - {field}"));
        }
        lines
    }
}

/// Fields the redaction/allowlist layer must strip before any event leaves
/// the process, regardless of destination (issue #13, step 4: "excluir
/// prompts, respostas, conteúdo, nomes de arquivos e caminhos completos").
/// Kept as a `const` so the diagnose output and any future schema validator
/// can share one source of truth.
pub const NEVER_SENT_FIELDS: &[&str] = &[
    "prompt_text",
    "completion_text",
    "file_contents",
    "file_path (absolute or project-relative)",
    "file_name",
    "tool_call_arguments",
    "tool_call_output",
    "raw_request_body",
    "raw_response_body",
];

/// Build a [`DiagnosticReport`] from the resolved telemetry mode and config.
/// Pure function — no I/O, no network. `mixpanel_configured` and
/// `otel_configured` are passed in explicitly (rather than re-parsing env)
/// so this stays trivially unit-testable and callers control what "would be
/// active" means for their process (e.g. build-time-baked tokens).
pub fn build_diagnostic_report(mode: TelemetryMode, config: &TelemetryConfig) -> DiagnosticReport {
    let opted_out = do_not_track_requested();
    let telemetry_active = !opted_out && !mode.is_disabled();
    let session_metrics_active = !opted_out && mode.session_metrics_enabled();
    let mixpanel_active =
        telemetry_active && config.mixpanel_enabled && config.mixpanel_token.is_some();
    let events_active = telemetry_active && config.events_url.is_some();
    let trace_upload_active = !opted_out && config.trace_upload.unwrap_or(mode.is_enabled());
    let otel_active = !opted_out && config.otel_enabled.unwrap_or(false);

    let reason_for = |active: bool, gate: &str| -> String {
        if opted_out {
            "DO_NOT_TRACK set".to_owned()
        } else if active {
            format!("{gate} enabled")
        } else {
            format!("{gate} disabled")
        }
    };

    let destinations = vec![
        DiagnosticDestination {
            id: "first_party_events",
            host: "events endpoint configured for this build (see `events_url`)".to_owned(),
            purpose: "aggregate product usage events (feature counts, session lifecycle)",
            active: events_active,
            reason: reason_for(events_active, "telemetry mode + events_url"),
        },
        DiagnosticDestination {
            id: "mixpanel",
            host: "api.mixpanel.com".to_owned(),
            purpose: "legacy product analytics — scheduled for removal per issue #13 \
                      (\"desabilitar destinos herdados por padrão\")",
            active: mixpanel_active,
            reason: reason_for(mixpanel_active, "mixpanel_enabled + mixpanel_token"),
        },
        DiagnosticDestination {
            id: "sentry",
            host: "configured Sentry DSN (build-time; empty unless explicitly baked in)".to_owned(),
            purpose: "crash and error reports, scrubbed of home dir/usernames/secrets",
            active: session_metrics_active,
            reason: reason_for(session_metrics_active, "telemetry mode"),
        },
        DiagnosticDestination {
            id: "trace_upload",
            host: "cli-chat-proxy.grok.com (GCS-backed trace/turn upload)".to_owned(),
            purpose: "session trace upload for debugging, opt-in beyond base telemetry",
            active: trace_upload_active,
            reason: reason_for(trace_upload_active, "trace_upload"),
        },
        DiagnosticDestination {
            id: "external_otel",
            host: config
                .otel_endpoint
                .clone()
                .unwrap_or_else(|| "(none configured)".to_owned()),
            purpose: "customer-configured OpenTelemetry export — off by default, requires \
                      double opt-in (GROK_EXTERNAL_OTEL + an explicit exporter)",
            active: otel_active,
            reason: reason_for(otel_active, "otel_enabled"),
        },
    ];

    DiagnosticReport {
        telemetry_mode: mode.to_string(),
        opted_out_via_do_not_track: opted_out,
        destinations,
        never_sent_fields: NEVER_SENT_FIELDS.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> TelemetryConfig {
        TelemetryConfig {
            enabled: None,
            events_url: None,
            events_api_key: None,
            mixpanel_token: None,
            mixpanel_enabled: false,
            trace_upload: None,
            otel_enabled: None,
            otel_metrics_exporter: None,
            otel_logs_exporter: None,
            otel_endpoint: None,
            otel_protocol: None,
            otel_log_user_prompts: None,
            otel_log_tool_details: None,
        }
    }

    #[test]
    fn disabled_mode_has_no_active_destinations() {
        let report = build_diagnostic_report(TelemetryMode::Disabled, &base_config());
        assert_eq!(report.active_destinations().count(), 0);
        assert_eq!(report.telemetry_mode, "false");
    }

    #[test]
    fn enabled_mode_with_mixpanel_configured_activates_mixpanel() {
        let mut cfg = base_config();
        cfg.mixpanel_enabled = true;
        cfg.mixpanel_token = Some("token".to_owned());
        let report = build_diagnostic_report(TelemetryMode::Enabled, &cfg);
        let mixpanel = report
            .destinations
            .iter()
            .find(|d| d.id == "mixpanel")
            .expect("mixpanel destination must be present");
        assert!(mixpanel.active);
    }

    #[test]
    fn enabled_mode_without_mixpanel_token_keeps_mixpanel_inactive() {
        let mut cfg = base_config();
        cfg.mixpanel_enabled = true; // enabled flag alone is not enough
        let report = build_diagnostic_report(TelemetryMode::Enabled, &cfg);
        let mixpanel = report
            .destinations
            .iter()
            .find(|d| d.id == "mixpanel")
            .unwrap();
        assert!(!mixpanel.active, "no token means nothing can be sent");
    }

    #[test]
    fn session_metrics_mode_activates_sentry_but_not_mixpanel() {
        let report = build_diagnostic_report(TelemetryMode::SessionMetrics, &base_config());
        let sentry = report
            .destinations
            .iter()
            .find(|d| d.id == "sentry")
            .unwrap();
        let mixpanel = report
            .destinations
            .iter()
            .find(|d| d.id == "mixpanel")
            .unwrap();
        assert!(sentry.active);
        assert!(!mixpanel.active);
    }

    #[test]
    #[allow(unsafe_code)]
    fn do_not_track_forces_every_destination_inactive_even_when_enabled() {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        unsafe { std::env::set_var("DO_NOT_TRACK", "1") };
        let mut cfg = base_config();
        cfg.mixpanel_enabled = true;
        cfg.mixpanel_token = Some("token".to_owned());
        cfg.trace_upload = Some(true);
        cfg.otel_enabled = Some(true);
        let report = build_diagnostic_report(TelemetryMode::Enabled, &cfg);
        assert!(report.opted_out_via_do_not_track);
        assert_eq!(
            report.active_destinations().count(),
            0,
            "DO_NOT_TRACK must zero out every destination regardless of config"
        );
        unsafe { std::env::remove_var("DO_NOT_TRACK") };
    }

    #[test]
    fn never_sent_fields_covers_prompt_and_path_content() {
        let report = build_diagnostic_report(TelemetryMode::Enabled, &base_config());
        assert!(report.never_sent_fields.contains(&"prompt_text"));
        assert!(report.never_sent_fields.contains(&"file_contents"));
        assert!(
            report
                .never_sent_fields
                .iter()
                .any(|f| f.contains("file_path"))
        );
    }

    #[test]
    fn to_text_lines_mentions_mode_and_all_destination_ids() {
        let report = build_diagnostic_report(TelemetryMode::Disabled, &base_config());
        let text = report.to_text_lines().join("\n");
        assert!(text.contains("Telemetry mode: false"));
        for dest in &report.destinations {
            assert!(text.contains(dest.id), "output must mention `{}`", dest.id);
        }
    }
}
