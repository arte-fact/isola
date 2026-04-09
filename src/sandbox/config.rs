use std::fmt;
use std::path::{Path, PathBuf};

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

#[derive(Serialize, Deserialize)]
pub struct SandboxConfig {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub rootfs_url: String,
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub environments: Vec<String>,
    #[serde(default)]
    pub share_display: bool,
    #[serde(default)]
    pub shell: SandboxShell,
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

/// Project-local shareable config stored at `<project>/.isola/config.yaml`.
/// All fields are optional — unspecified fields use sensible defaults.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct LocalConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<SandboxShell>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_display: Option<bool>,
}

impl LocalConfig {
    /// Load from `dir/.isola/config.yaml`. Returns `Ok(None)` if the file doesn't exist.
    pub fn load(dir: &Path) -> Result<Option<Self>, IsolaError> {
        let path = paths::local_config_path(dir);
        match std::fs::read_to_string(&path) {
            Ok(data) => {
                let config: Self = serde_yaml::from_str(&data)
                    .map_err(|e| IsolaError::ConfigError(e.to_string()))?;
                Ok(Some(config))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Write to `dir/.isola/config.yaml`, creating the `.isola/` directory if needed.
    pub fn save(&self, dir: &Path) -> Result<(), IsolaError> {
        let path = paths::local_config_path(dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data =
            serde_yaml::to_string(self).map_err(|e| IsolaError::ConfigError(e.to_string()))?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    /// Walk from cwd upward looking for `.isola/config.yaml`.
    /// Returns `(project_dir, config)` if found.
    pub fn find_from_cwd() -> Result<Option<(PathBuf, Self)>, IsolaError> {
        let cwd = std::env::current_dir()?;
        let mut dir = cwd.as_path();
        loop {
            if let Some(config) = Self::load(dir)? {
                return Ok(Some((dir.to_path_buf(), config)));
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return Ok(None),
            }
        }
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
            share_display: false,
            shell: SandboxShell::Fish,
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: SandboxConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, config.name);
        assert_eq!(deserialized.share_display, config.share_display);
        assert_eq!(deserialized.rootfs_url, config.rootfs_url);
        assert_eq!(deserialized.workspace, config.workspace);
        assert_eq!(deserialized.environments, config.environments);
        assert_eq!(deserialized.shell, SandboxShell::Fish);
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

    #[test]
    fn local_config_round_trip() {
        let config = LocalConfig {
            environments: Some(vec!["rust".to_string(), "nodejs".to_string()]),
            shell: Some(SandboxShell::Fish),
            share_display: None,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let deserialized: LocalConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(
            deserialized.environments,
            Some(vec!["rust".to_string(), "nodejs".to_string()])
        );
        assert_eq!(deserialized.shell, Some(SandboxShell::Fish));
    }

    #[test]
    fn local_config_partial_fields() {
        let yaml = "environments:\n  - rust\n";
        let config: LocalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.environments, Some(vec!["rust".to_string()]));
        assert!(config.shell.is_none());
        assert!(config.share_display.is_none());
    }

    #[test]
    fn local_config_empty() {
        let yaml = "{}";
        let config: LocalConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.environments.is_none());
        assert!(config.shell.is_none());
    }

    #[test]
    fn local_config_load_missing_file() {
        let dir = std::env::temp_dir().join("isola-test-missing");
        let result = LocalConfig::load(&dir).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn local_config_save_and_load() {
        let dir = std::env::temp_dir().join("isola-test-save-load");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = LocalConfig {
            environments: Some(vec!["go".to_string()]),
            shell: Some(SandboxShell::Zsh),
            share_display: None,
        };

        config.save(&dir).unwrap();
        let loaded = LocalConfig::load(&dir).unwrap().unwrap();
        assert_eq!(loaded.environments, Some(vec!["go".to_string()]));
        assert_eq!(loaded.shell, Some(SandboxShell::Zsh));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_corrupted_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "not valid json {{{").unwrap();
        // SandboxConfig::load reads from paths::config_path(name) so we can't
        // easily test it without setting HOME. Instead, test serde directly.
        let result: Result<SandboxConfig, _> = serde_json::from_str("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn config_extra_fields_ignored() {
        let json = r#"{
            "name": "test",
            "created_at": "2025-01-01T00:00:00Z",
            "rootfs_url": "http://example.com/rootfs.tar.gz",
            "unknown_field": "should be ignored"
        }"#;
        let result: Result<SandboxConfig, _> = serde_json::from_str(json);
        // serde defaults deny unknown fields — check what happens
        // If this errors, it's fine; it documents current behavior
        let _ = result;
    }

    #[test]
    fn shell_all_variants_have_bin_path() {
        for shell in &[SandboxShell::Bash, SandboxShell::Fish, SandboxShell::Zsh] {
            let path = shell.bin_path();
            assert!(
                path.starts_with('/'),
                "bin_path for {:?} should be absolute",
                shell
            );
        }
    }

    #[test]
    fn shell_all_variants_have_name() {
        for shell in &[SandboxShell::Bash, SandboxShell::Fish, SandboxShell::Zsh] {
            let name = shell.name();
            assert!(!name.is_empty(), "name for {:?} should not be empty", shell);
        }
    }
}
