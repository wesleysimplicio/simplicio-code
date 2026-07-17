//! Fixed allowlist of hosts that first-party telemetry/crash-report code is
//! permitted to contact.
//!
//! Scope: this module covers only the telemetry/analytics/crash-reporting
//! surface owned by this crate (`xai-grok-telemetry`) and its `xai-mixpanel`
//! dependency. It is **not** a whole-product network allowlist — the
//! product's core function (model inference, auth, session sync, updates)
//! contacts other hosts by design; see
//! `docs/privacy/network-destinations.md` for the full, product-wide
//! inventory and which parts are/aren't covered here.
//!
//! Two real telemetry-adjacent destinations are deliberately **not** in
//! [`ALLOWED_TELEMETRY_HOSTS`] because they cannot be pinned to a fixed
//! string, and are verified differently:
//!
//! - **Sentry**: the DSN is supplied at build time (`option_env!("SENTRY_DSN")`)
//!   or is simply absent (crash reporting is then a no-op — see
//!   `sentry.rs`). There is no compiled-in literal host to allowlist; the
//!   invariant this module can't check is instead "no DSN configured => no
//!   Sentry network call", which is exercised by
//!   `diagnostic_report`'s tests and by `sentry.rs` returning `None` when
//!   the DSN is empty.
//! - **External customer OpenTelemetry** (`OTEL_EXPORTER_OTLP_ENDPOINT` /
//!   `_LOGS_ENDPOINT` / `_METRICS_ENDPOINT`): opt-in, customer-configured by
//!   design (bring-your-own observability backend), gated by
//!   `GROK_EXTERNAL_OTEL`. Any host is legitimate here because the user
//!   supplied it; see `external/config.rs` and `external/redact.rs` for the
//!   fail-closed *content* allowlist that applies regardless of host.

/// Hosts that xAI's own first-party telemetry/analytics code (the product
/// events pipeline and Mixpanel) is allowed to contact by default. Anything
/// else contacted by this crate or `xai-mixpanel` without an explicit,
/// documented override is a bug.
///
/// - `api.mixpanel.com` — `xai-mixpanel`'s `track()`/`engage()` production
///   default (see `xai_mixpanel::DEFAULT_BASE_URL`).
/// - `cli-chat-proxy.grok.com` — the default product-events endpoint
///   (`GROK_TELEMETRY_BUILD_EVENTS_URL` / `GROK_TELEMETRY_EVENTS_URL`) and
///   the first-party session-metrics OTLP trace upload target (see
///   `otel_layer/mod.rs`); it is also the default chat/inference backend
///   (`xai-grok-env`), which is out of scope for this module but shares the
///   host.
pub const ALLOWED_TELEMETRY_HOSTS: &[&str] = &["api.mixpanel.com", "cli-chat-proxy.grok.com"];

/// True if `host` (as returned by [`url::Url::host_str`]) is one of the
/// documented, allowlisted telemetry destinations.
pub fn is_allowed_telemetry_host(host: &str) -> bool {
    ALLOWED_TELEMETRY_HOSTS.contains(&host)
}

/// Parses `url` and checks its host against [`ALLOWED_TELEMETRY_HOSTS`].
/// Fails closed: an unparsable URL or a URL with no host is treated as a
/// violation, same as an explicitly non-allowlisted host.
pub fn assert_allowed_telemetry_url(url: &str) -> Result<(), String> {
    let parsed =
        url::Url::parse(url).map_err(|e| format!("unparsable telemetry URL {url:?}: {e}"))?;
    match parsed.host_str() {
        Some(host) if is_allowed_telemetry_host(host) => Ok(()),
        Some(host) => Err(format!(
            "telemetry URL {url:?} targets non-allowlisted host {host:?}; \
             update ALLOWED_TELEMETRY_HOSTS and docs/privacy/network-destinations.md \
             if this is an intentional new destination"
        )),
        None => Err(format!("telemetry URL {url:?} has no host")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `xai-mixpanel`'s compiled-in production default must itself be
    /// allowlisted — otherwise the allowlist and the real default have
    /// drifted apart.
    #[test]
    fn mixpanel_production_default_is_allowlisted() {
        let host = url::Url::parse(xai_mixpanel::DEFAULT_BASE_URL)
            .unwrap()
            .host_str()
            .unwrap()
            .to_owned();
        assert!(
            is_allowed_telemetry_host(&host),
            "xai_mixpanel::DEFAULT_BASE_URL host {host:?} must be in ALLOWED_TELEMETRY_HOSTS"
        );
    }

    #[test]
    fn known_production_telemetry_urls_pass() {
        assert!(assert_allowed_telemetry_url("https://api.mixpanel.com/track").is_ok());
        assert!(assert_allowed_telemetry_url("https://api.mixpanel.com/engage").is_ok());
        assert!(assert_allowed_telemetry_url("https://cli-chat-proxy.grok.com/v1/events").is_ok());
        assert!(assert_allowed_telemetry_url("https://cli-chat-proxy.grok.com/v1/traces").is_ok());
    }

    #[test]
    fn unknown_host_is_rejected() {
        let err = assert_allowed_telemetry_url("https://telemetry-collector.evil.example/collect")
            .unwrap_err();
        assert!(err.contains("telemetry-collector.evil.example"), "{err}");
    }

    /// A near-miss/typosquat-shaped host (not a subdomain relationship the
    /// allowlist should ever treat as equal) must still be rejected — exact
    /// string match only, no accidental suffix/prefix matching.
    #[test]
    fn lookalike_host_is_rejected() {
        assert!(!is_allowed_telemetry_host("api.mixpanel.com.evil.example"));
        assert!(!is_allowed_telemetry_host("notapi.mixpanel.com"));
        assert!(!is_allowed_telemetry_host(
            "cli-chat-proxy.grok.com.attacker.net"
        ));
    }

    #[test]
    fn malformed_url_fails_closed() {
        assert!(assert_allowed_telemetry_url("not a url").is_err());
        assert!(assert_allowed_telemetry_url("file:///etc/passwd").is_err());
    }
}
