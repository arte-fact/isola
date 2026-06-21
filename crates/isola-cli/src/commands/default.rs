use std::path::Path;

use isola_core::error::IsolaError;
use isola_core::paths;
use isola_core::plugin::PluginRegistry;
use isola_core::sandbox::config::{LocalConfig, SandboxConfig, SandboxShell};

/// Default command: auto-detect sandbox for cwd, or launch setup wizard
pub fn run() -> Result<(), IsolaError> {
    match detect_sandbox()? {
        Some(name) => {
            eprintln!("Entering sandbox '{name}'...");
            let code = crate::commands::enter::run(&name, None, vec![])?;
            crate::reset_terminal();
            std::process::exit(code);
        }
        None => {
            // Check for a local .isola/config.yaml before falling back to the wizard
            if let Some((project_dir, local_config)) = LocalConfig::find_from_cwd()? {
                let name = create_from_local_config(&project_dir, &local_config)?;
                eprintln!("Entering sandbox '{name}'...");
                let code = crate::commands::enter::run(&name, None, vec![])?;
                crate::reset_terminal();
                std::process::exit(code);
            }
            crate::commands::setup::run()
        }
    }
}

/// Create a sandbox from a local `.isola/config.yaml`, deriving a unique name
/// from the project directory. This is the shared "app does the rest" path used
/// both when `isola` finds an existing config and right after the setup wizard
/// writes one.
pub(crate) fn create_from_local_config(
    project_dir: &Path,
    config: &LocalConfig,
) -> Result<String, IsolaError> {
    // Derive sandbox name from the project directory name
    let dir_name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "sandbox".to_string());

    // Sanitize: keep only valid characters
    let base_name: String = dir_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base_name = if base_name.is_empty() {
        "sandbox".to_string()
    } else {
        base_name
    };

    // Find a unique name (append suffix if needed)
    let name = if !paths::sandbox_dir(&base_name).exists() {
        base_name.clone()
    } else {
        let mut n = 2;
        loop {
            let candidate = format!("{base_name}-{n}");
            if !paths::sandbox_dir(&candidate).exists() {
                break candidate;
            }
            n += 1;
        }
    };

    let environments = config.environments.clone().unwrap_or_default();
    let shell = config
        .shell
        .clone()
        .unwrap_or_else(SandboxShell::detect_from_host);
    let share_display = config.share_display.unwrap_or(false);
    let plugin_vars = config.plugin_vars.clone().unwrap_or_default();
    let registry = PluginRegistry::load_for_project(Some(project_dir))?;

    eprintln!("Creating sandbox '{name}' from .isola/config.yaml...");

    crate::commands::create::run_with_envs(crate::commands::create::CreateRequest {
        name: &name,
        workspace: Some(project_dir.to_path_buf()),
        environments: &environments,
        share_display,
        no_cache: false,
        shell: &shell,
        registry: &registry,
        plugin_vars: &plugin_vars,
    })?;

    Ok(name)
}

/// Scan all sandbox configs to find one whose workspace matches cwd
pub fn detect_sandbox() -> Result<Option<String>, IsolaError> {
    let cwd = std::env::current_dir()?;
    find_sandbox_for_path(&cwd)
}

pub(crate) fn find_sandbox_for_path(cwd: &Path) -> Result<Option<String>, IsolaError> {
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
        if let Ok(config) = SandboxConfig::load(&name)
            && let Some(ws) = &config.workspace
            && (cwd == ws || cwd.starts_with(ws))
        {
            let depth = ws.components().count();
            if best_match.as_ref().is_none_or(|(_, d)| depth > *d) {
                best_match = Some((config.name, depth));
            }
        }
    }
    Ok(best_match.map(|(name, _)| name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_sandbox_no_sandboxes_dir() {
        // When sandboxes_dir doesn't exist, returns None (not an error)
        let result = find_sandbox_for_path(Path::new("/nonexistent/path"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
