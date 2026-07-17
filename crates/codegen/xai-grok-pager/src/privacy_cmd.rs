//! `simplicio-code privacy` — non-interactive privacy/telemetry controls.
//!
//! Complements the in-session `/privacy` slash command
//! (`src/slash/commands/privacy.rs`) with a command usable from scripts, CI,
//! and headless environments, per issue "privacy: telemetria mínima e
//! controles de dados": the user must be able to list exactly what would be
//! sent, and to disable telemetry before the very first event is emitted
//! (`privacy diagnose` never emits an event and never makes a network call
//! itself).

use anyhow::Result;
use clap::Subcommand;
use xai_grok_telemetry::diagnostic_report::build_diagnostic_report;

#[derive(Debug, clap::Args, Clone)]
pub struct PrivacyArgs {
    #[command(subcommand)]
    pub command: PrivacyCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum PrivacyCommand {
    /// List every telemetry/crash-report/trace destination and whether it is
    /// currently active, without sending anything.
    Diagnose {
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(args: PrivacyArgs) -> Result<()> {
    match args.command {
        PrivacyCommand::Diagnose { json } => run_diagnose(json).await,
    }
}

async fn run_diagnose(json: bool) -> Result<()> {
    let raw_config = xai_grok_shell::config::load_effective_config_disk_only()
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    let config = xai_grok_shell::agent::config::Config::new_from_toml_cfg(&raw_config)
        .map_err(|e| anyhow::anyhow!("Failed to parse config: {e}"))?;
    let mode = config.effective_telemetry_mode();
    let report = build_diagnostic_report(mode, &config.telemetry);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for line in report.to_text_lines() {
            println!("{line}");
        }
        println!();
        println!("Set DO_NOT_TRACK=1 (or GROK_TELEMETRY_ENABLED=0) to disable all telemetry.");
    }
    Ok(())
}
