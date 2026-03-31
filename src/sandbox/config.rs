use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::IsolaError;
use crate::paths;

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxShell {
    #[default]
    Bash,
    Fish,
    Zsh,
}

impl fmt::Display for SandboxShell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl SandboxShell {
    pub fn bin_path(&self) -> &str {
        match self {
            SandboxShell::Bash => "/bin/bash",
            SandboxShell::Fish => "/usr/bin/fish",
            SandboxShell::Zsh => "/usr/bin/zsh",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            SandboxShell::Bash => "bash",
            SandboxShell::Fish => "fish",
            SandboxShell::Zsh => "zsh",
        }
    }

    pub fn detect_from_host() -> Self {
        std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                let basename = std::path::Path::new(&s).file_name()?.to_str()?.to_string();
                match basename.as_str() {
                    "fish" => Some(SandboxShell::Fish),
                    "zsh" => Some(SandboxShell::Zsh),
                    "bash" => Some(SandboxShell::Bash),
                    _ => None,
                }
            })
            .unwrap_or(SandboxShell::Bash)
    }

    pub fn login_args(&self) -> Vec<String> {
        match self {
            SandboxShell::Bash => vec!["bash".to_string(), "-l".to_string()],
            SandboxShell::Fish => vec!["fish".to_string(), "-l".to_string()],
            SandboxShell::Zsh => vec!["zsh".to_string(), "-l".to_string()],
        }
    }
}

fn default_claude_enabled() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
pub struct SandboxConfig {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub rootfs_url: String,
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub environments: Vec<String>,
    #[serde(default)]
    pub share_ssh: bool,
    #[serde(default)]
    pub shell: SandboxShell,
    #[serde(default = "default_claude_enabled")]
    pub claude_integration: bool,
    #[serde(default)]
    pub install_neovim: bool,
}

impl SandboxConfig {
    pub fn load(name: &str) -> Result<Self, IsolaError> {
        let path = paths::config_path(name);
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data).map_err(|e| IsolaError::ConfigError(e.to_string()))
    }

    pub fn save(&self) -> Result<(), IsolaError> {
        let path = paths::config_path(&self.name);
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| IsolaError::ConfigError(e.to_string()))?;
        std::fs::write(&path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip_serialization() {
        let config = SandboxConfig {
            name: "test-sandbox".to_string(),
            created_at: chrono::Utc::now(),
            rootfs_url: "https://example.com/rootfs.tar.gz".to_string(),
            workspace: Some(std::path::PathBuf::from("/home/user/project")),
            environments: vec!["rust".to_string(), "nodejs".to_string()],
            share_ssh: true,
            shell: SandboxShell::Fish,
            claude_integration: false,
            install_neovim: true,
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: SandboxConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, config.name);
        assert_eq!(deserialized.share_ssh, config.share_ssh);
        assert_eq!(deserialized.rootfs_url, config.rootfs_url);
        assert_eq!(deserialized.workspace, config.workspace);
        assert_eq!(deserialized.environments, config.environments);
        assert_eq!(deserialized.shell, SandboxShell::Fish);
        assert!(!deserialized.claude_integration);
        assert!(deserialized.install_neovim);
    }

    #[test]
    fn config_without_optional_fields() {
        let json = r#"{
            "name": "minimal",
            "created_at": "2025-01-01T00:00:00Z",
            "rootfs_url": "https://example.com/rootfs.tar.gz",
            "workspace": null
        }"#;

        let config: SandboxConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "minimal");
        assert!(config.workspace.is_none());
        assert!(config.environments.is_empty());
        // Backward compat: missing fields get defaults
        assert_eq!(config.shell, SandboxShell::Bash);
        assert!(config.claude_integration); // true for backward compat
        assert!(!config.install_neovim);
    }

    #[test]
    fn config_with_environments() {
        let json = r#"{
            "name": "full",
            "created_at": "2025-01-01T00:00:00Z",
            "rootfs_url": "https://example.com/rootfs.tar.gz",
            "workspace": "/tmp/test",
            "environments": ["rust", "go"]
        }"#;

        let config: SandboxConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.environments, vec!["rust", "go"]);
    }

    #[test]
    fn shell_detect_from_env() {
        // Just test the enum methods directly
        assert_eq!(SandboxShell::Bash.bin_path(), "/bin/bash");
        assert_eq!(SandboxShell::Fish.bin_path(), "/usr/bin/fish");
        assert_eq!(SandboxShell::Zsh.bin_path(), "/usr/bin/zsh");
        assert_eq!(SandboxShell::Bash.name(), "bash");
        assert_eq!(SandboxShell::Fish.name(), "fish");
        assert_eq!(SandboxShell::Zsh.name(), "zsh");
    }

    #[test]
    fn shell_login_args() {
        assert_eq!(
            SandboxShell::Bash.login_args(),
            vec!["bash".to_string(), "-l".to_string()]
        );
        assert_eq!(
            SandboxShell::Fish.login_args(),
            vec!["fish".to_string(), "-l".to_string()]
        );
        assert_eq!(
            SandboxShell::Zsh.login_args(),
            vec!["zsh".to_string(), "-l".to_string()]
        );
    }
}
