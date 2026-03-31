use std::fmt;

use std::path::PathBuf;

use inquire::{Confirm, MultiSelect, Select, Text};

use crate::paths;
use crate::sandbox::config::SandboxShell;
use crate::sandbox::rootfs;

use crate::error::IsolaError;

#[derive(Clone)]
pub struct Environment {
    pub id: &'static str,
    pub label: &'static str,
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label)
    }
}

const AVAILABLE_ENVIRONMENTS: &[Environment] = &[
    Environment {
        id: "rust",
        label: "Rust (rustup + cargo)",
    },
    Environment {
        id: "nodejs",
        label: "Node.js 22 LTS (node + npm)",
    },
    Environment {
        id: "python-uv",
        label: "Python 3 + uv",
    },
    Environment {
        id: "go",
        label: "Go (latest)",
    },
];

/// Interactive setup wizard
pub fn run() -> Result<(), IsolaError> {
    crate::commands::create::preflight_checks()?;

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

    // 2. Environment selection
    let env_options: Vec<Environment> = AVAILABLE_ENVIRONMENTS.to_vec();
    let selected = MultiSelect::new("Select environments to install:", env_options)
        .with_help_message("Space to toggle, Enter to confirm")
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;

    let env_ids: Vec<String> = selected.iter().map(|e| e.id.to_string()).collect();

    if env_ids.is_empty() {
        eprintln!("No environments selected, installing base system only.");
    }

    // 3. Workspace confirmation
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let workspace = Text::new("Workspace directory:")
        .with_default(&cwd.to_string_lossy())
        .with_help_message("Host directory to mount at /workspace inside the sandbox")
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

    // 4. Share SSH keys?
    let host_ssh_dir = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".ssh"))
        .filter(|p| p.exists());
    let share_ssh = if host_ssh_dir.is_some() {
        Confirm::new("Share host SSH keys with the sandbox? (read-only)")
            .with_default(true)
            .with_help_message("Enables git push/pull over SSH inside the sandbox")
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?
    } else {
        false
    };

    // 5. Shell selection
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

    // 6. Neovim detection
    let host_has_neovim = rootfs::detect_neovim();
    let install_neovim = if host_has_neovim {
        Confirm::new("Neovim detected on host. Install in sandbox?")
            .with_default(true)
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?
    } else {
        false
    };

    // 7. Claude Code integration
    let claude_binary_available = crate::commands::enter::find_claude_binary().is_some();
    let claude_integration = if claude_binary_available {
        Confirm::new("Enable Claude Code integration?")
            .with_default(false)
            .with_help_message("Mounts Claude binary, credentials, and settings into the sandbox")
            .prompt()
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?
    } else {
        false
    };

    // 8. Create sandbox with selected options
    crate::commands::create::run_with_envs(
        &name,
        Some(workspace_path),
        &env_ids,
        share_ssh,
        false,
        &shell,
        claude_integration,
        install_neovim,
    )?;

    // 9. Optionally import host Claude config
    if claude_integration {
        import_host_config(&name)?;
    }

    // 10. Enter the sandbox
    if claude_integration {
        eprintln!("Launching Claude Code...");
        let code = crate::commands::enter::run(&name, false, Some(true), None)?;
        crate::reset_terminal();
        std::process::exit(code);
    } else {
        eprintln!("Launching {}...", shell.name());
        let code = crate::commands::enter::run(&name, false, None, None)?;
        crate::reset_terminal();
        std::process::exit(code);
    }
}

/// Offer to copy the host's Claude settings.json into the sandbox rootfs.
fn import_host_config(name: &str) -> Result<(), IsolaError> {
    let host_settings = paths::host_claude_settings();

    // Skip if host has no settings
    let host_data = match std::fs::read(&host_settings) {
        Ok(data) if !data.is_empty() => data,
        _ => return Ok(()),
    };

    let import = Confirm::new("Import Claude config from host? (keeps your settings/MCP servers)")
        .with_default(true)
        .prompt()
        .map_err(|e| IsolaError::ConfigError(e.to_string()))?;

    if import {
        let rootfs = paths::rootfs_dir(name);
        for target_dir in &["home/sandbox/.claude", "root/.claude"] {
            let target = rootfs.join(target_dir).join("settings.json");
            std::fs::create_dir_all(rootfs.join(target_dir))?;
            std::fs::write(&target, &host_data)?;
        }
        eprintln!("Config imported from {}", host_settings.display());
    }

    Ok(())
}
