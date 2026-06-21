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

    // 1. Shell — sourced from the shell-layer plugins (no hardcoded list)
    let detected_shell = SandboxShell::detect_from_host();
    let mut shell_names: Vec<String> = registry
        .plugins_for_layer(PluginLayer::Shell)
        .into_iter()
        .map(|p| p.manifest.name.clone())
        .collect();
    if shell_names.is_empty() {
        shell_names.push("bash".to_string());
    }
    let default_idx = shell_names
        .iter()
        .position(|s| s == detected_shell.name())
        .unwrap_or(0);
    let shell_refs: Vec<&str> = shell_names.iter().map(|s| s.as_str()).collect();
    let shell_choice = Select::new("Shell:", shell_refs)
        .with_starting_cursor(default_idx)
        .with_help_message(&format!(
            "Detected: {} — installed and configured in the sandbox",
            detected_shell.name()
        ))
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;
    let shell = SandboxShell::new(shell_choice);

    // 2. Display sharing (auto-detected)
    let has_display = std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
    let share_display = if has_display {
        Confirm::new("Share host display with the sandbox?")
            .with_default(true)
            .with_help_message(
                "Enables GUI apps and browser login, but shares your X11/Wayland \
                 socket (sandbox can see input/screen)",
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

    // 4. Project tooling: software to install in the sandbox (no defaults).
    // Annotate GPU plugins whose required device class isn't present on the host
    // (device-driven, so it works for any plugin that declares GPU devices).
    let has_nvidia = std::path::Path::new("/dev/nvidiactl").exists();
    let has_amd = std::path::Path::new("/dev/kfd").exists();
    let project_plugins: Vec<PluginChoice> = registry
        .plugins_for_layer(PluginLayer::Project)
        .into_iter()
        .map(|p| {
            let devs: Vec<&str> = p
                .manifest
                .paths
                .device
                .iter()
                .map(|d| d.path.as_str())
                .collect();
            let needs_nvidia = devs.iter().any(|d| d.contains("nvidia"));
            let needs_amd = devs.iter().any(|d| d.contains("kfd"));
            let mut description = p.manifest.description.clone();
            if (needs_nvidia || needs_amd)
                && !((needs_nvidia && has_nvidia) || (needs_amd && has_amd))
            {
                description.push_str("  — no compatible GPU detected on host");
            }
            PluginChoice {
                name: p.manifest.name.clone(),
                description,
            }
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

    // Install the selected shell plugin when it has an install script
    // (fish/zsh do; bash is part of the base and has none).
    if let Some(p) = registry.get(shell.name())
        && p.install_script.is_some()
        && !env_ids.contains(&shell.name().to_string())
    {
        env_ids.push(shell.name().to_string());
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

    // 6. Review before the (potentially long, network-heavy) provisioning.
    let import: Vec<&str> = selected_user.iter().map(|c| c.name.as_str()).collect();
    let install: Vec<&str> = selected_project.iter().map(|c| c.name.as_str()).collect();
    eprintln!("\nReady to set up {}:", project_dir.display());
    eprintln!("  Shell:   {}", shell.name());
    eprintln!(
        "  Display: {}",
        if share_display {
            "shared"
        } else {
            "not shared"
        }
    );
    eprintln!(
        "  Import:  {}",
        if import.is_empty() {
            "—".to_string()
        } else {
            import.join(", ")
        }
    );
    eprintln!(
        "  Install: {}",
        if install.is_empty() {
            "base system only".to_string()
        } else {
            install.join(", ")
        }
    );
    for (k, v) in &plugin_vars {
        eprintln!("  {k}={v}");
    }
    let proceed = Confirm::new("Write .isola/config.yaml and create the sandbox?")
        .with_default(true)
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;
    if !proceed {
        eprintln!("Cancelled — nothing was created.");
        return Ok(());
    }

    // 7. Author .isola/config.yaml — the wizard's only job. The app does the rest.
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

    // 8. Hand off to the standard config-driven create + enter path.
    let name = crate::commands::default::create_from_local_config(&project_dir, &local)?;
    eprintln!("Launching {}...", shell.name());
    let code = crate::commands::enter::run(&name, None, vec![])?;
    crate::reset_terminal();
    std::process::exit(code);
}
