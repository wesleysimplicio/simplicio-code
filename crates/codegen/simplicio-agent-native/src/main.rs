use simplicio_agent_native::{
    CapabilityManifest, DependencyStatus, DoctorReport, Health, ReasonCode,
};
use std::{env, path::PathBuf};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command] if command == "capabilities" => {
            serde_json::to_string_pretty(&CapabilityManifest::default())
        }
        [command, json] if command == "doctor" && json == "--json" => {
            serde_json::to_string_pretty(&doctor())
        }
        _ => {
            eprintln!("usage: simplicio-agent-native capabilities | doctor --json");
            std::process::exit(2);
        }
    };
    match result {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("failed to serialize protocol response: {error}");
            std::process::exit(1);
        }
    }
}

fn doctor() -> DoctorReport {
    DoctorReport::new(
        [
            ("agent", "SIMPLICIO_AGENT_BIN", "simplicio-agent"),
            ("loop_hub", "SIMPLICIO_LOOP_BIN", "simplicio-loop"),
            ("runtime", "SIMPLICIO_BIN", "simplicio"),
        ]
        .into_iter()
        .map(|(name, variable, binary)| dependency(name, variable, binary))
        .collect(),
        CapabilityManifest::default(),
    )
}

fn dependency(name: &str, variable: &str, binary: &str) -> DependencyStatus {
    let configured = env::var_os(variable).map(PathBuf::from);
    let found = configured.as_ref().is_some_and(|path| path.is_file())
        || env::var_os("PATH").is_some_and(|paths| {
            env::split_paths(&paths).any(|directory| directory.join(binary).is_file())
        });
    DependencyStatus {
        name: name.into(),
        health: if found {
            Health::Installed
        } else {
            Health::Missing
        },
        version: None,
        reason: (!found).then_some(ReasonCode::DependencyMissing),
        safe_command: (!found).then(|| {
            if cfg!(windows) {
                format!("where {binary}")
            } else {
                format!("command -v {binary}")
            }
        }),
    }
}
