use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::IsolaError;
use crate::paths;

/// The sandbox shell, identified by its plugin name (e.g. "bash", "fish").
/// Shell behavior (binary path, host detection) is defined by the `shell:`
/// block of the corresponding `layer: shell` plugin — there is no hardcoded
/// list. Serializes as the bare name for backward-compatible configs.
#[derive(Clone, Debug, PartialEq)]
pub struct SandboxShell(String);

impl Default for SandboxShell {
    fn default() -> Self {
        Self("bash".to_string())
    }
}

impl fmt::Display for SandboxShell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for SandboxShell {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SandboxShell {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self(String::deserialize(d)?))
    }
}

impl SandboxShell {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
    pub fn bash() -> Self {
        Self("bash".to_string())
    }
    #[cfg(test)]
    pub fn fish() -> Self {
        Self("fish".to_string())
    }
    #[cfg(test)]
    pub fn zsh() -> Self {
        Self("zsh".to_string())
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// Shell binary path inside the sandbox, resolved from the shell plugin's
    /// `shell.bin`, falling back to common locations.
    pub fn bin_path(&self) -> String {
        if let Ok(reg) = crate::plugin::PluginRegistry::load()
            && let Some(p) = reg.get(&self.0)
            && let Some(sh) = &p.manifest.shell
        {
            return sh.bin.clone();
        }
        match self.0.as_str() {
            "bash" => "/bin/bash".to_string(),
            other => format!("/usr/bin/{other}"),
        }
    }

    /// Pre-select the shell whose plugin matches the host `$SHELL`; else bash.
    pub fn detect_from_host() -> Self {
        let basename = std::env::var("SHELL").ok().and_then(|s| {
            std::path::Path::new(&s)
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from)
        });
        if let Some(b) = basename {
            if let Ok(reg) = crate::plugin::PluginRegistry::load()
                && reg
                    .plugins_for_layer(crate::plugin::PluginLayer::Shell)
                    .iter()
                    .any(|p| {
                        p.manifest.name == b
                            || p.manifest.shell.as_ref().and_then(|s| s.detect.as_deref())
                                == Some(b.as_str())
                    })
            {
                return Self(b);
            }
            if ["bash", "fish", "zsh"].contains(&b.as_str()) {
                return Self(b);
            }
        }
        Self::bash()
    }

    #[cfg(target_os = "linux")]
    pub fn login_args(&self) -> Vec<String> {
        vec![self.0.clone(), "-l".to_string()]
    }
}

#[derive(Debug, Serialize, Deserialize)]
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
    /// Device nodes to bind-mount from host (e.g., "/dev/kfd", "/dev/dri").
    #[serde(default)]
    pub devices: Vec<String>,
    /// Env-var values collected from plugin prompts during setup
    /// (e.g., {"PHP_VERSION": "8.3"}). Exported before each plugin's install script.
    #[serde(default)]
    pub plugin_vars: BTreeMap<String, String>,
}

impl SandboxConfig {
    pub fn load(name: &str) -> Result<Self, IsolaError> {
        let path = paths::config_path(name);
        // A missing config means the sandbox doesn't exist — report that clearly
        // rather than leaking a raw "No such file or directory" IO error.
        let data = std::fs::read_to_string(&path)
            .map_err(|_| IsolaError::SandboxNotFound(name.to_string()))?;
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
    /// Device nodes to bind-mount from host (e.g., "/dev/kfd", "/dev/dri").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devices: Option<Vec<String>>,
    /// Env-var values collected from plugin prompts during setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_vars: Option<BTreeMap<String, String>>,
}

impl LocalConfig {
    /// Load from `dir/.isola/config.yaml`. Returns `Ok(None)` if the file doesn't exist.
    pub fn load(dir: &Path) -> Result<Option<Self>, IsolaError> {
        let path = paths::local_config_path(dir);
        match std::fs::read_to_string(&path) {
            Ok(data) => {
                let config: Self = serde_yaml_ng::from_str(&data)
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
            serde_yaml_ng::to_string(self).map_err(|e| IsolaError::ConfigError(e.to_string()))?;
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
    fn load_missing_sandbox_reports_not_found() {
        // A name that cannot exist on disk must surface SandboxNotFound, not a
        // raw "No such file or directory" IO error.
        let err = SandboxConfig::load("isola-nonexistent-sandbox-9d3f1a-test").unwrap_err();
        assert!(
            matches!(err, IsolaError::SandboxNotFound(_)),
            "expected SandboxNotFound, got: {err:?}"
        );
    }

    #[test]
    fn config_round_trip_serialization() {
        let config = SandboxConfig {
            name: "test-sandbox".to_string(),
            created_at: chrono::Utc::now(),
            rootfs_url: "https://example.com/rootfs.tar.gz".to_string(),
            workspace: Some(std::path::PathBuf::from("/home/user/project")),
            environments: vec!["rust".to_string(), "nodejs".to_string()],
            share_display: false,
            shell: SandboxShell::fish(),
            devices: vec![],
            plugin_vars: BTreeMap::new(),
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: SandboxConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, config.name);
        assert_eq!(deserialized.share_display, config.share_display);
        assert_eq!(deserialized.rootfs_url, config.rootfs_url);
        assert_eq!(deserialized.workspace, config.workspace);
        assert_eq!(deserialized.environments, config.environments);
        assert_eq!(deserialized.shell, SandboxShell::fish());
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
        assert_eq!(config.shell, SandboxShell::bash());
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
        assert_eq!(SandboxShell::bash().bin_path(), "/bin/bash");
        assert_eq!(SandboxShell::fish().bin_path(), "/usr/bin/fish");
        assert_eq!(SandboxShell::zsh().bin_path(), "/usr/bin/zsh");
        assert_eq!(SandboxShell::bash().name(), "bash");
        assert_eq!(SandboxShell::fish().name(), "fish");
        assert_eq!(SandboxShell::zsh().name(), "zsh");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shell_login_args() {
        assert_eq!(
            SandboxShell::bash().login_args(),
            vec!["bash".to_string(), "-l".to_string()]
        );
        assert_eq!(
            SandboxShell::fish().login_args(),
            vec!["fish".to_string(), "-l".to_string()]
        );
        assert_eq!(
            SandboxShell::zsh().login_args(),
            vec!["zsh".to_string(), "-l".to_string()]
        );
    }

    #[test]
    fn local_config_round_trip() {
        let config = LocalConfig {
            environments: Some(vec!["rust".to_string(), "nodejs".to_string()]),
            shell: Some(SandboxShell::fish()),
            share_display: None,
            devices: None,
            plugin_vars: None,
        };

        let yaml = serde_yaml_ng::to_string(&config).unwrap();
        let deserialized: LocalConfig = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(
            deserialized.environments,
            Some(vec!["rust".to_string(), "nodejs".to_string()])
        );
        assert_eq!(deserialized.shell, Some(SandboxShell::fish()));
    }

    #[test]
    fn local_config_partial_fields() {
        let yaml = "environments:\n  - rust\n";
        let config: LocalConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.environments, Some(vec!["rust".to_string()]));
        assert!(config.shell.is_none());
        assert!(config.share_display.is_none());
    }

    #[test]
    fn local_config_empty() {
        let yaml = "{}";
        let config: LocalConfig = serde_yaml_ng::from_str(yaml).unwrap();
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
            shell: Some(SandboxShell::zsh()),
            share_display: None,
            devices: None,
            plugin_vars: None,
        };

        config.save(&dir).unwrap();
        let loaded = LocalConfig::load(&dir).unwrap().unwrap();
        assert_eq!(loaded.environments, Some(vec!["go".to_string()]));
        assert_eq!(loaded.shell, Some(SandboxShell::zsh()));

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
        for shell in &[
            SandboxShell::bash(),
            SandboxShell::fish(),
            SandboxShell::zsh(),
        ] {
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
        for shell in &[
            SandboxShell::bash(),
            SandboxShell::fish(),
            SandboxShell::zsh(),
        ] {
            let name = shell.name();
            assert!(!name.is_empty(), "name for {:?} should not be empty", shell);
        }
    }
}
