use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use inquire::{Confirm, MultiSelect, Select, Text};

use crate::error::IsolaError;
use crate::plugin::{PluginLayer, PluginRegistry};
use crate::sandbox::backend;
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
    let b = backend::create_backend();
    b.preflight_checks()?;

    let registry = PluginRegistry::load()?;

    // The wizard authors a project-local `.isola/config.yaml`; the app then
    // creates a sandbox named after — and mounting — the current directory.
    let project_dir = std::env::current_dir()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok())
        .unwrap_or_else(|| PathBuf::from("."));

    // 1. Shell selection (auto-detected from host)
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

    // 2. Display sharing (auto-detected)
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

    // 3. User setup: personal config from host (auto-detected defaults)
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

    // 4. Project tooling: software to install in the sandbox (no defaults)
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

    // 5. Collect plugin-declared prompt answers (e.g. PHP_VERSION for the php plugin)
    let mut plugin_vars: BTreeMap<String, String> = BTreeMap::new();
    for env in &env_ids {
        let Some(plugin) = registry.get(env) else {
            continue;
        };
        for prompt in &plugin.manifest.prompts {
            let default_str = prompt.default.as_deref();
            let answer = if prompt.choices.is_empty() {
                let mut text = Text::new(&prompt.message);
                if let Some(d) = default_str {
                    text = text.with_default(d);
                }
                text.prompt()
                    .map_err(|e| IsolaError::ConfigError(e.to_string()))?
            } else {
                let choices: Vec<&str> = prompt.choices.iter().map(|s| s.as_str()).collect();
                let cursor = default_str
                    .and_then(|d| choices.iter().position(|c| *c == d))
                    .unwrap_or(0);
                Select::new(&prompt.message, choices)
                    .with_starting_cursor(cursor)
                    .prompt()
                    .map_err(|e| IsolaError::ConfigError(e.to_string()))?
                    .to_string()
            };
            plugin_vars.insert(prompt.env_var.clone(), answer);
        }
    }

    // 6. Author .isola/config.yaml — the wizard's only job. The app does the rest.
    let local = LocalConfig {
        environments: if env_ids.is_empty() {
            None
        } else {
            Some(env_ids.clone())
        },
        shell: Some(shell.clone()),
        share_display: if share_display { Some(true) } else { None },
        devices: None,
        plugin_vars: if plugin_vars.is_empty() {
            None
        } else {
            Some(plugin_vars.clone())
        },
    };
    local.save(&project_dir)?;
    eprintln!("Wrote .isola/config.yaml — commit it to share this setup with your team.");

    // 7. Hand off to the standard config-driven create + enter path.
    let name = crate::commands::default::create_from_local_config(&project_dir, &local)?;
    eprintln!("Launching {}...", shell.name());
    let code = crate::commands::enter::run(&name, None, vec![])?;
    crate::reset_terminal();
    std::process::exit(code);
}
