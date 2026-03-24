use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::BotError;
use crate::paths;

#[derive(Serialize, Deserialize)]
pub struct SandboxConfig {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub rootfs_url: String,
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub environments: Vec<String>,
}

impl SandboxConfig {
    pub fn load(name: &str) -> Result<Self, BotError> {
        let path = paths::config_path(name);
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data).map_err(|e| BotError::ConfigError(e.to_string()))
    }

    pub fn save(&self) -> Result<(), BotError> {
        let path = paths::config_path(&self.name);
        let data =
            serde_json::to_string_pretty(self).map_err(|e| BotError::ConfigError(e.to_string()))?;
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
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: SandboxConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, config.name);
        assert_eq!(deserialized.rootfs_url, config.rootfs_url);
        assert_eq!(deserialized.workspace, config.workspace);
        assert_eq!(deserialized.environments, config.environments);
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
}
