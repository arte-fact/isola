use std::fmt;

use std::path::PathBuf;

use inquire::{Confirm, MultiSelect, Text};

use crate::error::BotError;

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
pub fn run() -> Result<(), BotError> {
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
        .map_err(|e| BotError::ConfigError(e.to_string()))?;

    // 2. Environment selection
    let env_options: Vec<Environment> = AVAILABLE_ENVIRONMENTS.to_vec();
    let selected = MultiSelect::new("Select environments to install:", env_options)
        .with_help_message("Space to toggle, Enter to confirm")
        .prompt()
        .map_err(|e| BotError::ConfigError(e.to_string()))?;

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
        .map_err(|e| BotError::ConfigError(e.to_string()))?;

    let workspace_path = PathBuf::from(&workspace);
    if !workspace_path.exists() {
        let create = Confirm::new(&format!("'{}' does not exist. Create it?", workspace))
            .with_default(true)
            .prompt()
            .map_err(|e| BotError::ConfigError(e.to_string()))?;
        if create {
            std::fs::create_dir_all(&workspace_path)?;
        }
    }

    // 4. Create sandbox with selected environments
    crate::commands::create::run_with_envs(&name, Some(workspace_path), &env_ids)?;

    // 6. Enter the sandbox
    eprintln!("Launching Claude Code...");
    let code = crate::commands::enter::run(&name, false, None)?;
    std::process::exit(code);
}
