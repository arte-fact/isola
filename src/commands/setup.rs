use std::fmt;
use std::path::PathBuf;

use inquire::{Confirm, MultiSelect, Select, Text};

use crate::error::IsolaError;
use crate::paths;
use crate::plugin::{PluginLayer, PluginRegistry};
use crate::sandbox::config::{LocalConfig, SandboxShell};

#[derive(Clone)]
struct PluginChoice {
    name: String,
    description: String,
}

impl fmt::Display for PluginChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description)
    }
}

/// Interactive setup wizard
pub fn run() -> Result<(), IsolaError> {
    crate::commands::create::preflight_checks()?;

    let registry = PluginRegistry::load()?;

    let dir_name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "sandbox".to_string());

    // 1. Sandbox name
    let name = Text::new("Sandbox name:")
        .with_default(&dir_name)
        .with_help_message("Name for the isolated environment")
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;

    // 2. Workspace confirmation
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let workspace = Text::new("Workspace directory:")
        .with_default(&cwd.to_string_lossy())
        .with_help_message("Host directory to mount inside the sandbox (at /<dirname>)")
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;

    let workspace_path = PathBuf::from(&workspace);
    if !workspace_path.exists() {
        let create = Confirm::new(&format!("'{}' does not exist. Create it?", workspace))
            .with_default(true)
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?;
        if create {
            std::fs::create_dir_all(&workspace_path)?;
        }
    }

    // 3. Shell selection (auto-detected from host)
    let detected_shell = SandboxShell::detect_from_host();
    let shell_options = vec!["bash", "fish", "zsh"];
    let default_idx = shell_options
        .iter()
        .position(|s| *s == detected_shell.name())
        .unwrap_or(0);
    let shell_choice = Select::new("Shell:", shell_options)
        .with_starting_cursor(default_idx)
        .with_help_message(&format!(
            "Detected: {} — will be installed and configured in the sandbox",
            detected_shell.name()
        ))
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;
    let shell = match shell_choice {
        "fish" => SandboxShell::Fish,
        "zsh" => SandboxShell::Zsh,
        _ => SandboxShell::Bash,
    };

    // 4. Display sharing (auto-detected)
    let has_display = std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
    let share_display = if has_display {
        Confirm::new("Share host display with the sandbox?")
            .with_default(true)
            .with_help_message(
                "Enables GUI apps, Chrome MCP, and Claude browser login inside the sandbox",
            )
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?
    } else {
        false
    };

    // 5. User Setup: personal config from host (auto-detected defaults)
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    let user_plugins: Vec<PluginChoice> = registry
        .plugins_for_layer(PluginLayer::User)
        .into_iter()
        .map(|p| PluginChoice {
            name: p.manifest.name.clone(),
            description: p.manifest.description.clone(),
        })
        .collect();

    let user_defaults: Vec<usize> = registry
        .plugins_for_layer(PluginLayer::User)
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let detected = p
                .manifest
                .auto_detect
                .as_ref()
                .map(|ad| {
                    home.as_ref()
                        .map(|h| h.join(&ad.host_path).exists())
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if detected { Some(i) } else { None }
        })
        .collect();

    let selected_user = if !user_plugins.is_empty() {
        MultiSelect::new("User setup — import from host:", user_plugins)
            .with_default(&user_defaults)
            .with_help_message(
                "Space to toggle, Enter to confirm — pre-selected = detected on host",
            )
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?
    } else {
        vec![]
    };

    // 6. Project Tooling: software to install in the sandbox (no defaults)
    let project_plugins: Vec<PluginChoice> = registry
        .plugins_for_layer(PluginLayer::Project)
        .into_iter()
        .map(|p| PluginChoice {
            name: p.manifest.name.clone(),
            description: p.manifest.description.clone(),
        })
        .collect();

    let selected_project = if !project_plugins.is_empty() {
        MultiSelect::new("Project tooling — install in sandbox:", project_plugins)
            .with_help_message("Space to toggle, Enter to confirm")
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?
    } else {
        vec![]
    };

    // Combine selected environments from both sections
    let mut env_ids: Vec<String> = selected_user
        .iter()
        .chain(selected_project.iter())
        .map(|e| e.name.clone())
        .collect();

    // Auto-add shell plugin based on shell choice
    match shell {
        SandboxShell::Fish => env_ids.push("fish".to_string()),
        SandboxShell::Zsh => env_ids.push("zsh".to_string()),
        SandboxShell::Bash => {}
    }

    if env_ids.is_empty() {
        eprintln!("No plugins selected, installing base system only.");
    }

    // 7. Create sandbox with selected options
    crate::commands::create::run_with_envs(
        &name,
        Some(workspace_path.clone()),
        &env_ids,
        share_display,
        false,
        &shell,
        &registry,
    )?;

    // 8. Save .isola/config.yaml for team sharing
    if !paths::local_config_path(&workspace_path).exists() {
        let local = LocalConfig {
            environments: Some(env_ids.clone()),
            shell: Some(shell.clone()),
            share_display: if share_display { Some(true) } else { None },
            devices: None,
        };
        local.save(&workspace_path)?;
        eprintln!("Saved .isola/config.yaml — commit to share sandbox config with your team.");
    }

    // 9. Enter the sandbox
    eprintln!("Launching {}...", shell.name());
    let code = crate::commands::enter::run(&name, None, vec![])?;
    crate::reset_terminal();
    std::process::exit(code);
}
