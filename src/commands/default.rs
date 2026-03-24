use std::path::Path;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;

/// Default command: auto-detect sandbox for cwd, or launch setup wizard
pub fn run() -> Result<(), IsolaError> {
    match detect_sandbox()? {
        Some(name) => {
            eprintln!("Entering sandbox '{name}'...");
            let code = crate::commands::enter::run(&name, false, None)?;
            std::process::exit(code);
        }
        None => crate::commands::setup::run(),
    }
}

/// Scan all sandbox configs to find one whose workspace matches cwd
pub fn detect_sandbox() -> Result<Option<String>, IsolaError> {
    let cwd = std::env::current_dir()?;
    find_sandbox_for_path(&cwd)
}

fn find_sandbox_for_path(cwd: &Path) -> Result<Option<String>, IsolaError> {
    let sandboxes_dir = paths::sandboxes_dir();
    if !sandboxes_dir.exists() {
        return Ok(None);
    }

    let mut best_match: Option<(String, usize)> = None;

    let entries = std::fs::read_dir(&sandboxes_dir)?;
    for entry in entries {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(config) = SandboxConfig::load(&name) {
            if let Some(ws) = &config.workspace {
                if cwd == ws || cwd.starts_with(ws) {
                    let depth = ws.components().count();
                    if best_match.as_ref().is_none_or(|(_, d)| depth > *d) {
                        best_match = Some((config.name, depth));
                    }
                }
            }
        }
    }
    Ok(best_match.map(|(name, _)| name))
}
